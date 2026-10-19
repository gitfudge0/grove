//! Native conversation surface for a structured Codex session.
use super::{
    components::{
        form_action, header_control, header_control_with_hover, header_control_with_tooltip,
        project_field_well, question_choice, question_disclosure, tooltip, tooltip_content,
        transcript_action,
    },
    rpx,
    tokens::*,
};
use crate::{entities::codex_session::CodexSession, theme as c};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use gpui::{
    div, prelude::*, AnimationExt as _, App, Context, Entity, FocusHandle, Focusable, Subscription,
    Window,
};
use gpui_component::{
    highlighter::HighlightTheme,
    input::{
        IndentInline, Input, InputEvent, InputState, MoveDown, MoveUp, TextDecoration,
        TextDecorationCollection,
    },
    menu::{PopupMenu, PopupMenuItem},
    text::{TextView, TextViewStyle},
    Disableable,
};
use grove_core::codex::{self, Action, SubmissionMode};
use grove_core::elicitation::{ElicitationKind, ElicitationMode, ElicitationRequest};
use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    fmt::Write as _,
    rc::Rc,
    sync::Arc,
};

const CHAT_MAX_W: f32 = 760.0;
const COMPOSER_MENU_W: f32 = 320.0;
const MINIMAP_PREVIEW_W: f32 = 320.0;
const COMPOSER_MENU_MAX_H: f32 = 280.0;
const COMPOSER_MIN_H: f32 = 72.0;
const USER_BUBBLE_MAX_W: f32 = 600.0;
const IMAGE_PREVIEW_EDGE: f32 = 56.0;
const COMPOSER_TOKEN_UNDERLINE: f32 = 1.0;
const TOOL_SHIMMER_PERIOD: std::time::Duration = std::time::Duration::from_millis(1600);
const TOOL_SHIMMER_WIDTH: f32 = 0.2;
const QUEUED_PROMPTS_MAX_H: f32 = 144.0;
const QUESTION_PANEL_MAX_H: f32 = 360.0;
const TOOL_DETAIL_MAX_H: f32 = 280.0;
/// Context share, in whole percent, at which the meter switches to the warning colour.
const CONTEXT_WARN_PERCENT: u32 = 80;
/// The context ring's stroke, the icon catalog's optical stroke.
const CONTEXT_RING_STROKE: f32 = 1.5;
/// Polyline segments per full turn of the context ring.
const CONTEXT_RING_SEGMENTS: f32 = 48.0;

/// A user input and the final (or latest streaming) assistant message before
/// the next input. Source indices stay distinct from rendered child indices.
#[derive(Debug, PartialEq)]
struct MessageMinimapEntry {
    item_index: usize,
    prompt: String,
    response: String,
}

fn message_minimap_entries(items: &[serde_json::Value]) -> Vec<MessageMinimapEntry> {
    let inputs: Vec<_> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item["type"] == "userMessage")
        .map(|(index, _)| index)
        .collect();
    inputs
        .iter()
        .enumerate()
        .map(|(turn, &index)| {
            let end = inputs.get(turn + 1).copied().unwrap_or(items.len());
            let replies: Vec<_> = items[index + 1..end]
                .iter()
                .filter(|item| item["type"] == "agentMessage")
                .filter(|item| !codex::item_text(item).trim().is_empty())
                .collect();
            let response = replies
                .iter()
                .rev()
                .find(|item| item["phase"] == "final_answer")
                .or_else(|| replies.last())
                .map_or_else(|| "No response yet".into(), |item| codex::item_text(item));
            let prompt = codex::item_text(&items[index]);
            MessageMinimapEntry {
                item_index: index,
                prompt: if prompt.trim().is_empty() {
                    "Image attachment".into()
                } else {
                    prompt
                },
                response,
            }
        })
        .collect()
}

fn minimap_excerpt(text: &str) -> String {
    // Bound tooltip layout and keep Unicode scalar boundaries intact.
    const PREVIEW_CHARACTERS: usize = 240;
    let mut characters = text
        .chars()
        .map(|character| {
            if character.is_whitespace() {
                ' '
            } else {
                character
            }
        })
        .take(PREVIEW_CHARACTERS + 1)
        .collect::<Vec<_>>();
    if characters.len() > PREVIEW_CHARACTERS {
        characters.truncate(PREVIEW_CHARACTERS);
        characters.push('…');
    }
    characters.into_iter().collect()
}

fn action_key(event: &gpui::KeyDownEvent) -> bool {
    matches!(event.keystroke.key.as_str(), "enter" | "space") && !event.is_held
}

fn composer_icon(
    id: &'static str,
    label: &'static str,
    icon: &'static str,
    disabled: bool,
    primary: bool,
) -> gpui::Stateful<gpui::Div> {
    header_control_with_hover(
        id,
        label,
        if disabled {
            if primary {
                c::FG()
            } else {
                c::FIELD_FILL()
            }
        } else if primary {
            c::FG_DIM()
        } else {
            c::BG_HOVER()
        },
    )
    .debug_selector(move || id.into())
    .size(rpx(CHROME_CONTROL_H))
    .rounded_full()
    .tab_index(0)
    .tab_stop(!disabled)
    .when(primary, |button| button.bg(c::FG()))
    .aria_description(if disabled { "Unavailable" } else { label })
    .when(!disabled, |button| {
        button.focus_visible(|style| style.border_1().border_color(c::SEL_RING()))
    })
    .when(disabled, |button| button.opacity(OPACITY_DISABLED))
    .child(crate::icons::icon(
        icon,
        ICON_SM,
        if primary { c::BG() } else { c::FG_DIM() },
    ))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ComposerMenu {
    Model,
    Effort,
    Access,
}

#[derive(Clone)]
enum ElicitationChoice {
    Include,
    Boolean,
    Option(String, bool),
}

struct ElicitationFieldDraft {
    input: Entity<InputState>,
    selected: HashSet<String>,
    choice_focus: HashMap<String, FocusHandle>,
    included: bool,
    boolean: bool,
    _subscription: Subscription,
}
struct ElicitationDraft {
    fields: Vec<ElicitationFieldDraft>,
    submitting: bool,
}

struct ApprovalDraft {
    input: Entity<InputState>,
    feedback: Entity<InputState>,
    editing: bool,
    submitting: bool,
    _subscriptions: Vec<Subscription>,
}

struct QuestionDraft {
    selected: HashSet<usize>,
    choice_focus: Vec<FocusHandle>,
    custom: bool,
    input: Entity<InputState>,
    _subscription: Subscription,
}

#[derive(Clone, Debug)]
struct QuestionReceipt {
    anchor: usize,
    title: Option<String>,
    answers: Vec<(String, String)>,
}

struct QuestionGroupDraft {
    questions: Vec<QuestionDraft>,
    expanded: Option<usize>,
    row_focus: Vec<FocusHandle>,
    anchor: usize,
    ack_after: u64,
    turn_bound: bool,
    pending_receipt: Option<QuestionReceipt>,
    submitting: bool,
}
fn question_has_answer(question: &codex::Question, answers: &codex::QuestionAnswers) -> bool {
    answers
        .get(&question.id)
        .is_some_and(|values| values.iter().any(|value| !value.trim().is_empty()))
}

fn question_answer_summary(question: &codex::Question, answers: &codex::QuestionAnswers) -> String {
    if !question_has_answer(question, answers) {
        "Not answered".into()
    } else if question.secret {
        "Provided securely".into()
    } else {
        let mut summary = answers[&question.id]
            .iter()
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        if let codex::QuestionKind::Number(number) = &question.kind {
            if let Some(unit) = &number.unit {
                let _ = write!(summary, " {unit}");
            }
        }
        summary
    }
}

fn question_key(state: &codex::Snapshot, request: &codex::QuestionRequest) -> String {
    format!(
        "{}:{}:{}",
        state.saved.thread_id.as_deref().unwrap_or_default(),
        if request.turn_bound() {
            state.turn_id.as_deref().unwrap_or_default()
        } else {
            ""
        },
        request.id
    )
}
/// Every conversation attachment uses the transcript/composer's centered content rail.
fn content_rail(child: impl IntoElement) -> gpui::Div {
    div()
        .w_full()
        .flex_shrink_0()
        .flex()
        .justify_center()
        .px(rpx(SPACE_20))
        .child(div().w_full().min_w_0().max_w(rpx(CHAT_MAX_W)).child(child))
}
/// Attached backing shared by completion choices and queued messages.
fn composer_tray(window: &Window) -> gpui::Div {
    div()
        .mx(rpx(
            if window.viewport_size().width < gpui::px(CHAT_MAX_W) {
                SPACE_2XL
            } else {
                SPACE_20
            },
        ))
        .mb(-rpx(SPACE_2XL + SPACE_MD))
        .pb(rpx(SPACE_2XL + SPACE_MD))
        .px(rpx(SPACE_2XL))
        .pt(rpx(SPACE_LG))
        .rounded_t(rpx(SPACE_20))
        .border_1()
        .border_color(c::BORDER_SOFT())
        .bg(c::BG_RAIL())
        .flex()
        .flex_col()
}

/// Keep decoded image dimensions out of attachment layout in both message surfaces.
fn attachment_preview(image: Arc<gpui::Image>) -> gpui::Div {
    div()
        .relative()
        .flex_shrink_0()
        .size(rpx(IMAGE_PREVIEW_EDGE))
        .overflow_hidden()
        .rounded(rpx(RADIUS_GROUP))
        .bg(c::BG())
        .child(
            gpui::img(image)
                .absolute()
                .inset_0()
                .w_full()
                .h_full()
                .min_w_0()
                .min_h_0()
                .object_fit(gpui::ObjectFit::Contain),
        )
}

#[derive(Clone)]
struct ComposerToken {
    value: String,
    decoration: TextDecorationCollection,
}

/// A local send can be acknowledged before its transcript item arrives.
struct BubbleEntrance {
    submission: u64,
    existing: HashSet<String>,
    thread: Option<String>,
    text: String,
}

struct QueueEditor {
    id: u64,
    draft: String,
    images: Vec<String>,
    tokens: Vec<ComposerToken>,
    loaded: bool,
    saving: bool,
}
struct QueueMotionRow {
    prompt: codex::QueuedPrompt,
    started: std::time::Instant,
    exiting: bool,
}

pub struct CodexView {
    session: Entity<CodexSession>,
    input: Entity<InputState>,
    checkout_context: Option<(String, String)>,
    expanded: HashSet<String>,
    work_tick_pending: Rc<Cell<bool>>,
    question_drafts: HashMap<String, QuestionGroupDraft>,
    question_receipts: Vec<QuestionReceipt>,
    question_generation: u64,
    approval_drafts: HashMap<String, ApprovalDraft>,
    elicitation_drafts: HashMap<String, ElicitationDraft>,
    interaction_scope: String,
    elicitation_thread: String,
    menu: Option<ComposerMenu>,
    popup: Option<Entity<PopupMenu>>,
    popup_subscription: Option<Subscription>,
    option_bounds: [Rc<Cell<gpui::Bounds<gpui::Pixels>>>; 3],
    option_focus: [FocusHandle; 3],
    file_suggestions: Vec<String>,
    picked_tokens: Vec<ComposerToken>,
    suggestion_index: usize,
    file_query: String,
    file_lookup: Option<String>,
    dismissed_suggestions: Option<String>,
    submitted: Option<String>,
    submitted_ack: Option<u64>,
    pending_clear: Option<(u64, String, Vec<String>)>,
    queue_selected: Option<u64>,
    queue_collapsed: bool,
    queue_editor: Option<QueueEditor>,
    queue_motion_rows: Vec<QueueMotionRow>,
    bubble_entrances: Vec<BubbleEntrance>,
    animating_bubbles: HashMap<String, (std::time::Instant, Rc<Cell<bool>>)>,
    interrupt_requested_for_turn: Option<String>,
    images: Vec<String>,
    previews: HashMap<String, Arc<gpui::Image>>,
    pending_image_pastes: usize,
    scroll: gpui::ScrollHandle,
    last_content: String,
    smooth_follow: bool,
    last_follow_offset: Option<gpui::Pixels>,
    last_follow_max: gpui::Pixels,
    motion_frame: Option<std::time::Instant>,
    motion_frame_pending: Rc<Cell<bool>>,
    stream_text: HashMap<String, (String, usize)>,
    stream_thread: Option<String>,
    follow_after_submit: bool,
    composer_collapsed: bool,
    pending_composer_scroll: Option<bool>,
    composer_pin_bottom: bool,
    composer_body_height: Rc<Cell<gpui::Pixels>>,
    composer_body_target: Rc<Cell<gpui::Pixels>>,
    composer_expanded_height: Rc<Cell<gpui::Pixels>>,
    composer_transition_from: Option<gpui::Pixels>,
    composer_transition_generation: usize,
    composer_motion_finished: Rc<Cell<bool>>,
    _subscriptions: Vec<Subscription>,
}
impl CodexView {
    pub fn new(session: Entity<CodexSession>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let stream_thread = session.read(cx).state.saved.thread_id.clone();
        let draft = session.read(cx).state.saved.draft.clone();
        let images = session.read(cx).state.saved.draft_images.clone();
        let previews = images
            .iter()
            .filter_map(|url| image_from_url(url).map(|image| (url.clone(), image)))
            .collect();
        let placeholder = composer_placeholder(session.read(cx).state.saved.provider);
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .auto_grow(3, 10)
                .accept_image_paste(true)
                .submit_on_enter(true)
                .placeholder(placeholder)
                .default_value(draft)
        });
        let subscriptions = vec![
            cx.observe(&session, |_, _, cx| cx.notify()),
            cx.observe(&input, |this, _, cx| {
                this.validate_picked_tokens(cx);
                this.refresh_files(cx);
                cx.notify();
            }),
            cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
                InputEvent::PressEnter {
                    secondary: true,
                    shift: false,
                } => {
                    if this.session.read(cx).state.status.busy() {
                        this.submit_mode(SubmissionMode::Steer, window, cx);
                    } else {
                        this.submit(window, cx);
                    }
                }
                InputEvent::PressEnter {
                    secondary: false,
                    shift: false,
                } => this.submit(window, cx),
                InputEvent::PasteImages(images) => this.paste_images(images.clone(), window, cx),
                InputEvent::Focus => this.set_composer_collapsed(false, cx),
                InputEvent::Change => {
                    this.set_composer_collapsed(false, cx);
                    let text = this.input.read(cx).value().to_string();
                    if this.queue_editor.is_none() {
                        this.queue_selected = None;
                        this.session
                            .update(cx, |session, cx| session.send(Action::Draft(text), cx));
                    }
                    this.dismissed_suggestions = None;
                    this.suggestion_index = 0;
                    this.validate_picked_tokens(cx);
                    this.refresh_files(cx);
                    cx.notify();
                }
                _ => {}
            }),
        ];
        Self {
            session,
            input,
            expanded: HashSet::new(),
            work_tick_pending: Rc::new(Cell::new(false)),
            question_drafts: HashMap::new(),
            question_receipts: Vec::new(),
            question_generation: 0,
            approval_drafts: HashMap::new(),
            elicitation_drafts: HashMap::new(),
            interaction_scope: String::new(),
            elicitation_thread: String::new(),
            menu: None,
            popup: None,
            popup_subscription: None,
            option_bounds: std::array::from_fn(|_| Rc::default()),
            option_focus: std::array::from_fn(|_| cx.focus_handle()),
            file_suggestions: Vec::new(),
            picked_tokens: Vec::new(),
            suggestion_index: 0,
            file_query: String::new(),
            file_lookup: None,
            dismissed_suggestions: None,
            submitted: None,
            submitted_ack: None,
            pending_clear: None,
            queue_selected: None,
            queue_collapsed: false,
            queue_editor: None,
            queue_motion_rows: Vec::new(),
            bubble_entrances: Vec::new(),
            animating_bubbles: HashMap::new(),
            interrupt_requested_for_turn: None,
            images,
            previews,
            pending_image_pastes: 0,
            scroll: gpui::ScrollHandle::new(),
            last_content: String::new(),
            smooth_follow: false,
            last_follow_offset: None,
            last_follow_max: gpui::Pixels::ZERO,
            motion_frame: None,
            motion_frame_pending: Rc::default(),
            stream_text: HashMap::new(),
            stream_thread,
            follow_after_submit: true,
            composer_collapsed: false,
            pending_composer_scroll: None,
            composer_pin_bottom: false,
            composer_body_height: Rc::default(),
            composer_body_target: Rc::default(),
            composer_expanded_height: Rc::default(),
            composer_transition_from: None,
            composer_transition_generation: 0,
            composer_motion_finished: Rc::new(Cell::new(true)),
            checkout_context: None,
            _subscriptions: subscriptions,
        }
    }
    pub(crate) fn set_checkout_context(
        &mut self,
        location: String,
        branch: String,
        cx: &mut Context<Self>,
    ) {
        let context = Some((location, branch));
        if self.checkout_context != context {
            self.checkout_context = context;
            cx.notify();
        }
    }

    fn use_starter(&mut self, prompt: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.input.read(cx).value().to_string();
        let draft = if current.is_empty() {
            prompt.to_owned()
        } else {
            format!("{current}\n\n{prompt}")
        };
        self.input
            .update(cx, |input, cx| input.set_value(draft.clone(), window, cx));
        self.session
            .update(cx, |session, cx| session.send(Action::Draft(draft), cx));
        self.set_composer_collapsed(false, cx);
        self.input.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn launch_intro(&self, state: &codex::Snapshot) -> gpui::Stateful<gpui::Div> {
        let (location, branch) = self
            .checkout_context
            .clone()
            .unwrap_or_else(|| (state.saved.cwd.clone(), String::new()));
        let identity = grove_core::session_meta::multi_project_identity(
            &state.saved.project,
            &state.saved.roots,
        )
        .unwrap_or_else(|| state.saved.project.clone());
        div()
            .id("codex-empty")
            .debug_selector(|| "codex-empty".into())
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG))
            .mb(rpx(SPACE_2XL))
            .child(
                div()
                    .id("codex-launch-context")
                    .debug_selector(|| "codex-launch-context".into())
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_wrap()
                    .gap(rpx(SPACE_2XL))
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .mb(rpx(SPACE_2XL))
                    .children(
                        [identity, location, branch]
                            .into_iter()
                            .filter(|value| !value.is_empty())
                            .enumerate()
                            .map(|(index, value)| {
                                let full_value = value.clone();
                                div()
                                    .id(("codex-launch-identity", index))
                                    .min_w_0()
                                    .max_w_full()
                                    .flex_shrink_0()
                                    .whitespace_normal()
                                    .aria_label(value.clone())
                                    .when(index > 0, |value| {
                                        value
                                            .border_l_1()
                                            .border_color(c::BORDER_SOFT())
                                            .pl(rpx(SPACE_2XL))
                                    })
                                    .tooltip(move |window, cx| {
                                        tooltip(full_value.clone(), window).build(window, cx)
                                    })
                                    .child(value)
                            }),
                    ),
            )
            .child(
                div()
                    .text_size(rpx(TEXT_DISPLAY))
                    .child("What would you like to build?"),
            )
            .child(
                div()
                    .text_color(c::FG_DIM())
                    .child("Ask a question, describe a change, or paste an image."),
            )
    }

    fn launch_starters(&self, provider: &str, cx: &Context<Self>) -> gpui::Div {
        let mut starters = div()
            .id("codex-launch-starters")
            .debug_selector(|| "codex-launch-starters".into())
            .w_full()
            .flex()
            .flex_wrap()
            .gap(rpx(SPACE_LG))
            .mt(rpx(SPACE_LG));
        for (id, title, description, prompt) in [
            ("codex-starter-understand", "Understand this project", "Map the code and explain where to start", "Explain the main modules in this project and where I should start."),
            ("codex-starter-change", "Make a change", "Describe what should work differently", "Help me make a change in this project. Ask me what should work differently before editing."),
            ("codex-starter-review", "Review my changes", "Look for bugs in the current diff", "Review my uncommitted changes for bugs and regressions."),
        ] {
            starters = starters.child(header_control(id, title).debug_selector(move || id.into())
                .tab_index(0).flex_1().flex_basis(rpx(CHAT_MAX_W / 3.0 - SPACE_LG)).min_w_0().h_auto()
                .p(rpx(SPACE_2XL)).border_1().border_color(c::BORDER_SOFT())
                .rounded(rpx(RADIUS_CONTROL)).flex().flex_col().items_start().justify_start().gap(rpx(SPACE_SM))
                .focus_visible(|style| style.bg(c::BG_HOVER()).border_color(c::BORDER_STRONG()))
                .aria_description(format!("{description}. Adds an editable prompt without sending."))
                .child(div().id(gpui::SharedString::from(format!("{id}-title"))).debug_selector(move || format!("{id}-title")).text_color(c::FG()).child(title))
                .child(div().whitespace_normal().text_color(c::FG_DIM()).child(description))
                .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if action_key(event) { this.use_starter(prompt, window, cx); cx.stop_propagation(); }
                }))
                .on_click(cx.listener(move |this, _, window, cx| this.use_starter(prompt, window, cx))));
        }
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_2XL))
            .child(starters)
            .child(
                div()
                    .text_size(rpx(TEXT_MICRO))
                    .text_color(c::FG_DIM())
                    .child(format!(
                        "{provider} works in this worktree using your CLI sign-in."
                    )),
            )
    }

    fn sync_bubble_entrances(
        &mut self,
        state: &codex::Snapshot,
        reduced_motion: bool,
        now: std::time::Instant,
    ) {
        self.animating_bubbles.retain(|_, (started, finished)| {
            !finished.get()
                && !reduced_motion
                && now.duration_since(*started) < std::time::Duration::from_millis(MOTION_SLOW_MS)
        });
        let mut consumed = HashSet::new();
        self.bubble_entrances.retain_mut(|pending| {
            if pending
                .thread
                .as_ref()
                .is_some_and(|thread| state.saved.thread_id.as_ref() != Some(thread))
                || state.rejected_submission == Some(pending.submission)
                || state.status == codex::Status::Disconnected
            {
                return false;
            }
            pending.existing.extend(consumed.iter().cloned());
            let id = state.saved.items.iter().find_map(|item| {
                let id = item["id"].as_str()?;
                (item["type"] == "userMessage"
                    && !pending.existing.contains(id)
                    && !consumed.contains(id)
                    && codex::item_text(item) == pending.text)
                    .then_some(id)
            });
            if let Some(id) = id {
                consumed.insert(id.to_owned());
                if !reduced_motion {
                    self.animating_bubbles
                        .insert(id.to_owned(), (now, Rc::new(Cell::new(false))));
                }
                false
            } else {
                true
            }
        });
    }
    fn queue_action(&mut self, action: Action, cx: &mut Context<Self>) -> bool {
        match self.session.read(cx).handle.send(action) {
            Ok(()) => true,
            Err(error) => {
                self.session.update(cx, |session, cx| {
                    session.state.error = Some(error);
                    cx.notify();
                });
                false
            }
        }
    }
    fn begin_queue_edit(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.queue_editor.is_some()
            || self.submitted.is_some()
            || self.pending_image_pastes > 0
            || !self.session.read(cx).state.queue_editable(id)
        {
            return;
        }
        let editor = QueueEditor {
            id,
            draft: self.input.read(cx).value().to_string(),
            images: self.images.clone(),
            tokens: self.picked_tokens.clone(),
            loaded: false,
            saving: false,
        };
        let result = self.queue_action(Action::BeginQueuedEdit(id), cx);
        if result {
            self.queue_editor = Some(editor);
            self.queue_selected = Some(id);
        }
        cx.notify();
    }
    fn save_queue_edit(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self
            .queue_editor
            .as_mut()
            .filter(|editor| editor.loaded && !editor.saving)
        else {
            return;
        };
        let text = self.input.read(cx).value().to_string();
        if let Err(error) = codex::validate_prompt(&text, &self.images) {
            self.session.update(cx, |session, cx| {
                session.state.error = Some(error);
                cx.notify();
            });
            return;
        }
        let id = editor.id;
        if self.queue_action(
            Action::SaveQueuedEdit {
                id,
                text,
                images: self.images.clone(),
            },
            cx,
        ) {
            if let Some(editor) = &mut self.queue_editor {
                editor.saving = true;
            }
        }
        cx.notify();
    }
    fn restore_queue_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.queue_editor.take() {
            self.images = editor.images;
            for url in &self.images {
                if !self.previews.contains_key(url) {
                    if let Some(image) = image_from_url(url) {
                        self.previews.insert(url.clone(), image);
                    }
                }
            }
            self.picked_tokens = editor.tokens;
            self.input.update(cx, |input, cx| {
                input.set_value(editor.draft.clone(), window, cx);
            });
            self.session.update(cx, |session, cx| {
                session.send(Action::Draft(editor.draft), cx);
                session.send(Action::DraftImages(self.images.clone()), cx);
            });
            self.queue_selected = None;
            self.input.focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }
    fn cancel_queue_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = &self.queue_editor {
            if editor.saving {
                return;
            }
            let id = editor.id;
            if !self.queue_action(Action::CancelQueuedEdit(id), cx) {
                return;
            }
        }
        self.restore_queue_draft(window, cx);
    }
    fn sync_queue_editor(
        &mut self,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = &mut self.queue_editor {
            if !editor.loaded && state.queue_edit == Some(editor.id) {
                if let Some(prompt) = state
                    .queued_prompts
                    .iter()
                    .find(|prompt| prompt.id == editor.id)
                {
                    // Capture the current unrelated draft at the moment the hold is granted.
                    // Pending input/attachment changes must survive the editor handoff.
                    editor.draft = self.input.read(cx).value().to_string();
                    editor.images = self.images.clone();
                    editor.tokens = self.picked_tokens.clone();
                    editor.loaded = true;
                    self.images = prompt.images.clone();
                    for url in &self.images {
                        if !self.previews.contains_key(url) {
                            if let Some(image) = image_from_url(url) {
                                self.previews.insert(url.clone(), image);
                            }
                        }
                    }
                    self.picked_tokens.clear();
                    self.input.update(cx, |input, cx| {
                        input.set_value(prompt.text.clone(), window, cx);
                    });
                    self.input.focus_handle(cx).focus(window, cx);
                }
            } else if editor.saving && state.queue_edit != Some(editor.id) {
                self.restore_queue_draft(window, cx);
            } else if !editor.loaded
                && !state.queue_editable(editor.id)
                && state.queue_edit != Some(editor.id)
            {
                // The provider won the dispatch race. Never overwrite the draft with a stale row.
                self.queue_editor = None;
                self.queue_selected = None;
            }
        }
        if self
            .queue_selected
            .is_some_and(|id| !state.queued_prompts.iter().any(|prompt| prompt.id == id))
        {
            self.queue_selected = None;
        }
    }
    fn move_queue_selection(
        &mut self,
        up: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.input.focus_handle(cx).is_focused(window)
            || self.queue_editor.is_some()
            || !self.input.read(cx).value().is_empty()
            || self.submitted.is_some()
        {
            return false;
        }
        let state = &self.session.read(cx).state;
        let ids = state
            .queued_prompts
            .iter()
            .filter(|prompt| state.queue_editable(prompt.id))
            .map(|prompt| prompt.id)
            .collect::<Vec<_>>();
        let current = self
            .queue_selected
            .and_then(|id| ids.iter().position(|candidate| *candidate == id));
        self.queue_selected = match (up, current) {
            (true, None) => ids.last().copied(),
            (true, Some(index)) => ids.get(index.saturating_sub(1)).copied(),
            (false, Some(index)) => ids.get(index + 1).copied(),
            (false, None) => return false,
        };
        if ids.is_empty() {
            return false;
        }
        self.queue_collapsed = false;
        cx.notify();
        true
    }
    fn sync_queue_motion(&mut self, state: &codex::Snapshot, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        let reduced = cx.reduce_motion();
        self.queue_motion_rows.retain(|row| {
            !row.exiting
                || (!reduced
                    && now.duration_since(row.started)
                        < std::time::Duration::from_millis(MOTION_BASE_MS))
        });
        let mut animate = false;
        for row in &mut self.queue_motion_rows {
            if let Some(prompt) = state
                .queued_prompts
                .iter()
                .find(|prompt| prompt.id == row.prompt.id)
            {
                row.prompt = prompt.clone();
            } else if !row.exiting {
                row.exiting = true;
                row.started = now;
                animate = true;
            }
        }
        for prompt in &state.queued_prompts {
            if !self
                .queue_motion_rows
                .iter()
                .any(|row| row.prompt.id == prompt.id)
            {
                self.queue_motion_rows.push(QueueMotionRow {
                    prompt: prompt.clone(),
                    started: now,
                    exiting: false,
                });
                animate = true;
            }
        }
        if reduced {
            self.queue_motion_rows.retain(|row| !row.exiting);
        }
        if animate && !reduced {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(MOTION_BASE_MS))
                    .await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
    }
    fn queue_tray(
        &mut self,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if self.queue_motion_rows.is_empty() {
            return div().into_any_element();
        }
        let count = state
            .queued_prompts
            .iter()
            .filter(|prompt| !state.queue_steering.contains(&prompt.id))
            .count();
        let has_steering = !state.queue_steering.is_empty();
        let pending_count = if has_steering {
            state.queued_prompts.len()
        } else {
            count
        };
        let collapsed = self.queue_collapsed;
        let mut tray = composer_tray(window)
            .id("codex-queued-prompts")
            .debug_selector(|| "codex-queued-prompts".into())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .child(crate::icons::icon("queue", ICON_MD, c::FG_MUTE()))
                    .child(
                        div()
                            .flex_1()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(if has_steering { "Pending" } else { "Queued" }),
                    )
                    .child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_MUTE())
                            .child(pending_count.to_string()),
                    )
                    .when(!state.status.busy() && count > 0, |header| {
                        header.child(
                            header_control("codex-resume-queue", "Send next queued message")
                                .tab_index(0)
                                .tab_stop(
                                    state.status != codex::Status::Disconnected
                                        && state.queue_uncertain.is_empty(),
                                )
                                .child(crate::icons::icon("arrow-up", ICON_MD, c::FG_DIM()))
                                .on_key_down(cx.listener(
                                    |this, event: &gpui::KeyDownEvent, _, cx| {
                                        if action_key(event) {
                                            this.queue_action(Action::ResumeQueue, cx);
                                            cx.stop_propagation();
                                        }
                                    },
                                ))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.queue_action(Action::ResumeQueue, cx);
                                })),
                        )
                    })
                    .child(
                        header_control(
                            "codex-collapse-queue",
                            if collapsed {
                                "Expand queued messages"
                            } else {
                                "Collapse queued messages"
                            },
                        )
                        .tab_index(0)
                        .focus_visible(|style| style.bg(c::BG_HOVER()))
                        .child(crate::icons::icon(
                            if collapsed { "chev-right" } else { "chev-down" },
                            ICON_MD,
                            c::FG_MUTE(),
                        ))
                        .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                            if action_key(event) {
                                this.queue_collapsed = !this.queue_collapsed;
                                cx.notify();
                                cx.stop_propagation();
                            }
                        }))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.queue_collapsed = !this.queue_collapsed;
                            cx.notify();
                        })),
                    ),
            );
        if !collapsed {
            let mut rows = div()
                .id("codex-queue-rows")
                .max_h(rpx(QUEUED_PROMPTS_MAX_H))
                .overflow_y_scroll()
                .flex()
                .flex_col();
            for motion_row in &self.queue_motion_rows {
                let prompt = &motion_row.prompt;
                let id = prompt.id;
                let exiting = motion_row.exiting;
                let locked = exiting || !state.queue_editable(id) || self.queue_editor.is_some();
                let steer = !locked
                    && state.saved.provider == codex::Provider::Codex
                    && state.status.busy()
                    && state.turn_id.is_some();
                let selected = self.queue_selected == Some(id);
                let label = if prompt.images.is_empty() {
                    collapsed_draft_preview(&prompt.text)
                } else {
                    format!(
                        "{} · {} images",
                        collapsed_draft_preview(&prompt.text),
                        prompt.images.len()
                    )
                };
                let number = state
                    .queued_prompts
                    .iter()
                    .position(|prompt| prompt.id == id)
                    .map(|index| (index + 1).to_string())
                    .unwrap_or_default();
                let mut row = div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .pl(rpx(SPACE_LG))
                    .rounded(rpx(RADIUS_CONTROL))
                    .when(selected, |row| row.bg(c::MENU_HOVER()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(rpx(TEXT_SMALL))
                            .child(label),
                    );
                if state.queue_edit == Some(id) {
                    row = row.child(
                        div()
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_DIM())
                            .child("Editing"),
                    );
                } else if state.queue_uncertain.contains(&id) {
                    row = row.child(
                        div()
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_DIM())
                            .child("Delivery unconfirmed"),
                    );
                    row = row.child(
                        form_action("codex-dismiss-uncertain", "Dismiss", false, window, cx)
                            .tooltip("Check the conversation first. Dismiss removes this unconfirmed entry without sending it again.")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.queue_action(Action::DismissUncertainQueued(id), cx);
                            })),
                    );
                } else if state.queue_sending.contains(&id) {
                    row = row.child(
                        div()
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_MUTE())
                            .child(if state.queue_steering.contains(&id) {
                                "Steering…"
                            } else {
                                "Sending…"
                            }),
                    );
                }
                row = row
                    .child(
                        header_control("codex-edit-queued", "Edit queued message")
                            .tab_index(0)
                            .tab_stop(!locked)
                            .focus_visible(|style| style.bg(c::BG_HOVER()))
                            .child(crate::icons::icon(
                                "edit",
                                ICON_MD,
                                if locked { c::FG_MUTE() } else { c::FG_DIM() },
                            ))
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, _, cx| {
                                    if action_key(event) && !locked {
                                        this.begin_queue_edit(id, cx);
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !locked {
                                    this.begin_queue_edit(id, cx);
                                }
                            })),
                    )
                    .when(state.saved.provider == codex::Provider::Codex, |row| {
                        row.child(
                            header_control("codex-steer-queued", "Steer into the current turn")
                                .w_auto()
                                .px(rpx(SPACE_SM))
                                .tab_index(0)
                                .tab_stop(steer)
                                .focus_visible(|style| style.bg(c::BG_HOVER()))
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_SM))
                                .child(crate::icons::icon(
                                    "steer",
                                    ICON_MD,
                                    if steer { c::FG_DIM() } else { c::FG_MUTE() },
                                ))
                                .child(
                                    div()
                                        .text_size(rpx(TEXT_SMALL))
                                        .text_color(if steer { c::FG_DIM() } else { c::FG_MUTE() })
                                        .child("Steer"),
                                )
                                .on_key_down(cx.listener(
                                    move |this, event: &gpui::KeyDownEvent, _, cx| {
                                        if action_key(event) && steer {
                                            this.queue_action(Action::SteerQueued(id), cx);
                                            cx.stop_propagation();
                                        }
                                    },
                                ))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if steer {
                                        this.queue_action(Action::SteerQueued(id), cx);
                                    }
                                })),
                        )
                    })
                    .child(
                        header_control("codex-remove-queued", "Remove queued message")
                            .tab_index(0)
                            .tab_stop(!locked)
                            .focus_visible(|style| style.bg(c::BG_HOVER()))
                            .child(crate::icons::icon(
                                "close",
                                ICON_MD,
                                if locked { c::FG_MUTE() } else { c::FG_DIM() },
                            ))
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, _, cx| {
                                    if action_key(event) && !locked {
                                        this.queue_action(Action::RemoveQueued(id), cx);
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !locked {
                                    this.queue_action(Action::RemoveQueued(id), cx);
                                }
                            })),
                    );
                let row = div()
                    .id(format!("codex-queued-{id}"))
                    .debug_selector(move || format!("codex-queued-{id}"))
                    .h(rpx(CHROME_CONTROL_H + SPACE_SM))
                    .flex_shrink_0()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .child(
                        div()
                            .w(rpx(ICON_SM))
                            .flex_shrink_0()
                            .text_center()
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_MUTE())
                            .child(number),
                    )
                    .child(row);
                let elapsed = motion_row.started.elapsed();
                let row = if !cx.reduce_motion()
                    && elapsed < std::time::Duration::from_millis(MOTION_BASE_MS)
                {
                    let exiting = motion_row.exiting;
                    row.with_animation(
                        format!("queue-row-motion-{id}-{exiting}"),
                        gpui::Animation::new(std::time::Duration::from_millis(MOTION_BASE_MS))
                            .with_easing(super::motion::ease_standard),
                        move |row, progress| {
                            let fraction = if exiting { 1.0 - progress } else { progress };
                            row.h(rpx((CHROME_CONTROL_H + SPACE_SM) * fraction))
                                .opacity(fraction)
                        },
                    )
                    .into_any_element()
                } else {
                    row.into_any_element()
                };
                rows = rows.child(row);
            }
            tray = tray.child(rows);
        }
        if self
            .queue_editor
            .as_ref()
            .is_some_and(|editor| editor.loaded)
        {
            let saving = self
                .queue_editor
                .as_ref()
                .is_some_and(|editor| editor.saving);
            tray = tray.child(
                div()
                    .flex()
                    .justify_end()
                    .gap(rpx(SPACE_SM))
                    .child(
                        header_control("codex-cancel-queue-edit", "Cancel queued message edit")
                            .debug_selector(|| "codex-cancel-queue-edit".into())
                            .tab_index(0)
                            .tab_stop(!saving)
                            .focus_visible(|style| style.bg(c::BG_HOVER()))
                            .child(crate::icons::icon(
                                "close",
                                ICON_MD,
                                if saving { c::FG_MUTE() } else { c::FG_DIM() },
                            ))
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, window, cx| {
                                    if action_key(event) && !saving {
                                        this.cancel_queue_edit(window, cx);
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if !saving {
                                    this.cancel_queue_edit(window, cx);
                                }
                            })),
                    )
                    .child(
                        header_control("codex-save-queue-edit", "Save queued message edit")
                            .debug_selector(|| "codex-save-queue-edit".into())
                            .tab_index(0)
                            .tab_stop(!saving)
                            .focus_visible(|style| style.bg(c::BG_HOVER()))
                            .child(crate::icons::icon(
                                "check",
                                ICON_MD,
                                if saving { c::FG_MUTE() } else { c::FG_DIM() },
                            ))
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, _, cx| {
                                    if action_key(event) && !saving {
                                        this.save_queue_edit(cx);
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !saving {
                                    this.save_queue_edit(cx);
                                }
                            })),
                    ),
            );
        }
        tray.into_any_element()
    }
    fn draft_action_pending(&self, cx: &App) -> bool {
        let state = &self.session.read(cx).state;
        self.submitted.is_some()
            || self.pending_image_pastes > 0
            || matches!(
                state.status,
                codex::Status::Starting | codex::Status::Disconnected
            )
            || (state.status.busy()
                && state.turn_id.is_some()
                && self.interrupt_requested_for_turn.as_ref() == state.turn_id.as_ref())
    }
    fn set_composer_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        if !collapsed {
            self.pending_composer_scroll = None;
        }
        if self.composer_collapsed == collapsed {
            return;
        }
        self.composer_transition_from =
            (!cx.reduce_motion()).then(|| self.composer_body_height.get());
        self.composer_transition_generation = self.composer_transition_generation.wrapping_add(1);
        self.composer_motion_finished.set(cx.reduce_motion());
        self.composer_collapsed = collapsed;
        self.input.update(cx, |input, cx| {
            if collapsed {
                input.set_auto_grow(1, 1, cx);
            } else {
                input.set_auto_grow(3, 10, cx);
            }
        });
        cx.notify();
    }

    fn jump_to_message(&mut self, child: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.smooth_follow = false;
        self.follow_after_submit = false;
        self.composer_pin_bottom = false;
        self.pending_composer_scroll = None;
        self.set_composer_collapsed(true, cx);
        // GPUI resolves this against the next layout, including any composer resize.
        self.scroll.scroll_to_top_of_item(child);
        let owner = cx.entity().downgrade();
        window.on_next_frame(move |_, cx| {
            let _ = owner.update(cx, |_, cx| cx.notify());
        });
        cx.notify();
    }

    fn render_message_minimap(
        &self,
        entries: &[MessageMinimapEntry],
        anchors: &[(usize, usize)],
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if entries.is_empty() {
            return div().into_any_element();
        }
        let near_bottom = self.scroll.max_offset().y + self.scroll.offset().y
            <= rpx(SPACE_20).to_pixels(window.rem_size());
        let active = if near_bottom {
            anchors.last()
        } else {
            anchors
                .iter()
                .rev()
                .find(|(_, child)| *child <= self.scroll.top_item())
                .or_else(|| anchors.first())
        }
        .map(|(index, _)| *index);
        let preview_width = (self.scroll.bounds().size.width
            - rpx(SPACE_3XL).to_pixels(window.rem_size()))
        .max(rpx(CHROME_CONTROL_H).to_pixels(window.rem_size()))
        .min(rpx(MINIMAP_PREVIEW_W - SPACE_3XL).to_pixels(window.rem_size()));
        let rail = div()
            .id("codex-message-minimap")
            .debug_selector(|| "codex-message-minimap".into())
            .role(gpui::Role::Group)
            .aria_label("Conversation messages")
            .w(rpx(CHROME_CONTROL_H))
            .flex_shrink_0()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .py(rpx(SPACE_3XL));
        // Auto margins center short stacks and resolve to zero on overflow.
        // Override the shared header control's fixed height and flex basis so
        // markers form compact rows, compressing only to their minimum hit area.
        let marker_count = anchors.len() as f32;
        let mut stack = div()
            .w_full()
            .flex()
            .flex_col()
            .my_auto()
            .h(rpx(marker_count * SPACE_LG))
            .min_h(rpx(marker_count * SPACE_SM))
            .flex_shrink_1();
        for entry in entries {
            let Some((_, child)) = anchors.iter().find(|(index, _)| *index == entry.item_index)
            else {
                continue;
            };
            let child = *child;
            let index = entry.item_index;
            let selected = active == Some(index);
            let prompt = minimap_excerpt(&entry.prompt);
            let response = minimap_excerpt(&entry.response);
            stack = stack.child(
                header_control_with_tooltip(
                    "codex-minimap-message",
                    "Go to message",
                    move |window, cx| {
                        let prompt = prompt.clone();
                        let response = response.clone();
                        tooltip_content(
                            move |_, _| {
                                div()
                                    .w(preview_width)
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(rpx(SPACE_LG))
                                    .whitespace_normal()
                                    .child(div().text_color(c::FG()).child(prompt.clone()))
                                    .child(div().text_color(c::FG_DIM()).child(response.clone()))
                            },
                            window,
                        )
                        .bg(c::SURFACE_RAISED())
                        .border_color(c::BORDER())
                        .build(window, cx)
                    },
                )
                .id(("codex-minimap-message", index))
                .debug_selector(move || format!("codex-minimap-message-{index}"))
                .w_full()
                .h(rpx(SPACE_LG))
                .flex_1()
                .flex_basis(rpx(SPACE_LG))
                .flex_shrink_1()
                .min_h(rpx(SPACE_SM))
                .max_h(rpx(SPACE_LG))
                .tab_index(0)
                .aria_label(format!("Go to message: {}", entry.prompt))
                .aria_description(format!("Response: {}", entry.response))
                .focus_visible(|style| style.bg(c::BG_HOVER()))
                .child(
                    div()
                        .w(rpx(if selected { SPACE_3XL } else { SPACE_LG }))
                        .h(rpx(SPACE_XS))
                        .bg(if selected { c::FG() } else { c::FG_MUTE() }),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.jump_to_message(child, window, cx);
                }))
                .on_key_down(cx.listener(
                    move |this, event: &gpui::KeyDownEvent, window, cx| {
                        if action_key(event) {
                            this.jump_to_message(child, window, cx);
                            cx.stop_propagation();
                        }
                    },
                )),
            );
        }
        rail.child(stack).into_any_element()
    }

    fn transcript_scrolled(&mut self, toward_bottom: bool, cx: &mut Context<Self>) {
        // The next render observes the final offset after native wheel dispatch.
        // Keep this separate from layout/content updates, which never enter browse mode.
        self.smooth_follow = false;
        self.pending_composer_scroll = Some(toward_bottom);
        self.composer_pin_bottom = false;
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.queue_editor.is_some() {
            self.save_queue_edit(cx);
            return;
        }
        if let Some(id) = self.queue_selected {
            self.begin_queue_edit(id, cx);
            return;
        }
        if self.submitted.is_some() {
            return;
        }
        if self.run_command(window, cx) {
            return;
        }
        if self.draft_action_pending(cx) {
            return;
        }
        let mode = if self.session.read(cx).state.status.busy() {
            SubmissionMode::Queue
        } else {
            SubmissionMode::Send
        };
        self.submit_mode(mode, window, cx);
    }
    fn interrupt(&mut self, cx: &mut Context<Self>) {
        let state = &self.session.read(cx).state;
        if !state.status.busy()
            || state.turn_id.is_none()
            || self.interrupt_requested_for_turn.as_ref() == state.turn_id.as_ref()
        {
            return;
        }
        self.interrupt_requested_for_turn = state.turn_id.clone();
        self.session.update(cx, |session, cx| {
            session.state.error = None;
            session.send(Action::Interrupt, cx);
        });
        if self.session.read(cx).state.error.is_some() {
            self.interrupt_requested_for_turn = None;
        }
        cx.notify();
    }
    fn submit_mode(&mut self, mode: SubmissionMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.queue_editor.is_some() {
            self.save_queue_edit(cx);
            return;
        }
        if self.submitted.is_none() && self.run_command(window, cx) {
            return;
        }
        let state = &self.session.read(cx).state;
        if self.draft_action_pending(cx)
            || (mode == SubmissionMode::Steer
                && (!state.status.busy()
                    || state.saved.provider != codex::Provider::Codex
                    || state.turn_id.is_none()))
        {
            return;
        }
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() && self.images.is_empty() {
            return;
        }
        let result = self.session.update(cx, |session, cx| {
            session.submit_with_mode(text.clone(), self.images.clone(), mode, cx)
        });
        if let Ok(submission) = result {
            self.bubble_entrances.push(BubbleEntrance {
                submission,
                existing: self
                    .session
                    .read(cx)
                    .state
                    .saved
                    .items
                    .iter()
                    .filter_map(|item| item["id"].as_str().map(str::to_owned))
                    .collect(),
                thread: self.session.read(cx).state.saved.thread_id.clone(),
                text: text.clone(),
            });
            self.submitted = Some(text);
            self.submitted_ack = Some(submission);
            self.follow_after_submit = true;
        } else if let Err(error) = result {
            self.session.update(cx, |session, cx| {
                session.state.error = Some(error);
                cx.notify();
            });
        }
    }
}
impl Focusable for CodexView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}
impl Render for CodexView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.restore_image_tokens(window, cx);
        let state = self.session.read(cx).state.clone();
        self.sync_queue_editor(&state, window, cx);
        self.sync_queue_motion(&state, cx);
        if self
            .pending_clear
            .as_ref()
            .is_some_and(|(generation, _, _)| state.cleared_conversations > *generation)
        {
            if let Some((_, draft, images)) = self.pending_clear.take() {
                if self.input.read(cx).value().as_ref() == draft {
                    self.input
                        .update(cx, |input, cx| input.set_value("", window, cx));
                    self.picked_tokens.clear();
                    self.file_suggestions.clear();
                    self.file_query.clear();
                    self.file_lookup = None;
                    self.dismissed_suggestions = None;
                }
                if self.images == images {
                    self.images.clear();
                }
            }
            self.submitted = None;
            self.submitted_ack = None;
            self.expanded.clear();
            self.menu = None;
            self.popup = None;
            self.popup_subscription = None;
            self.interrupt_requested_for_turn = None;
            self.bubble_entrances.clear();
            self.animating_bubbles.clear();
            self.approval_drafts.clear();
            self.question_drafts.clear();
            self.question_receipts.clear();
            self.elicitation_drafts.clear();
            self.follow_after_submit = true;
        }
        let provider = state.saved.provider.label();
        let scope = format!("{:?}:{:?}", state.saved.thread_id, state.turn_id);
        if self.interaction_scope != scope {
            let restore_focus = self.approval_drafts.values().any(|draft| {
                draft.input.focus_handle(cx).is_focused(window)
                    || draft.feedback.focus_handle(cx).is_focused(window)
            });
            self.approval_drafts.clear();
            self.interaction_scope = scope;
            if restore_focus {
                self.input.focus_handle(cx).focus(window, cx);
            }
        }
        // Standalone MCP requests can outlive a turn; retain them while their request remains live.
        let thread = format!("{:?}", state.saved.thread_id);
        if self.elicitation_thread != thread {
            let restore_focus = self.elicitation_drafts.values().any(|draft| {
                draft.fields.iter().any(|field| {
                    field.input.focus_handle(cx).is_focused(window)
                        || field
                            .choice_focus
                            .values()
                            .any(|focus| focus.is_focused(window))
                })
            });
            self.elicitation_drafts.clear();
            self.elicitation_thread = thread;
            if restore_focus {
                self.input.focus_handle(cx).focus(window, cx);
            }
        }
        self.sync_bubble_entrances(&state, cx.reduce_motion(), cx.background_executor().now());
        self.sync_questions(&state, window, cx);
        self.sync_approvals(&state, window, cx);
        self.sync_elicitations(&state, window, cx);
        if self
            .submitted_ack
            .is_some_and(|target| state.accepted_submission == Some(target))
        {
            self.submitted = None;
            self.submitted_ack = None;
            self.input
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.images.clear();
        }
        if self
            .submitted_ack
            .is_some_and(|id| state.rejected_submission == Some(id))
            || state.status == codex::Status::Disconnected
        {
            self.submitted = None;
            self.submitted_ack = None;
        }
        // Tool output/status and same-length streaming replacements also change layout.
        let content = serde_json::to_string(&state.saved.items).unwrap_or_default();
        let near_bottom = self.scroll.max_offset().y + self.scroll.offset().y
            <= rpx(SPACE_20).to_pixels(window.rem_size());
        if let Some(toward_bottom) = self.pending_composer_scroll.take() {
            // Collapsing enlarges the transcript viewport and can clamp its offset to
            // the new bottom. That layout change must not reverse an upward gesture.
            let restoring_bottom = toward_bottom && near_bottom && self.composer_collapsed;
            if restoring_bottom || (!toward_bottom && !near_bottom) {
                self.set_composer_collapsed(!restoring_bottom, cx);
            }
            if restoring_bottom {
                self.composer_pin_bottom = true;
                self.smooth_follow = true;
            }
        }
        if self.follow_after_submit {
            self.composer_pin_bottom = true;
            self.set_composer_collapsed(false, cx);
        }
        let changed_thread = self.stream_thread != state.saved.thread_id;
        if changed_thread {
            self.stream_text.clear();
            self.stream_thread.clone_from(&state.saved.thread_id);
        }
        if cx.reduce_motion() {
            // A later preference toggle must not resurrect an older partial prefix.
            for (text, shown) in self.stream_text.values_mut() {
                *shown = text.len();
            }
        }
        let initial_transcript = self.last_content.is_empty() || changed_thread;
        let stream_keys: HashSet<_> = state
            .saved
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                item["id"]
                    .as_str()
                    .map_or_else(|| format!("message-{index}"), str::to_owned)
            })
            .collect();
        self.stream_text.retain(|key, _| stream_keys.contains(key));
        if self.follow_after_submit {
            self.last_follow_offset = None;
        }
        let now = cx.background_executor().now();
        let elapsed = self
            .motion_frame
            .replace(now)
            .map_or(0.0, |previous| now.duration_since(previous).as_secs_f32());
        if self.follow_after_submit
            || (content != self.last_content
                && (near_bottom || self.smooth_follow)
                && !self.composer_collapsed)
        {
            self.smooth_follow = true;
            self.follow_after_submit = false;
        }
        if self.scroll.max_offset().y == self.last_follow_max
            && self
                .last_follow_offset
                .is_some_and(|offset| offset != self.scroll.offset().y)
            && !near_bottom
        {
            self.smooth_follow = false;
        }
        if self.smooth_follow {
            let target = -self.scroll.max_offset().y;
            let current = self.scroll.offset();
            let fraction = if cx.reduce_motion() {
                1.0
            } else {
                1.0 - (-elapsed / (MOTION_FAST_MS as f32 / 1000.0)).exp()
            };
            let next = current.y + (target - current.y) * fraction;
            let settled = (target - next).abs() <= gpui::px(0.5);
            self.scroll
                .set_offset(gpui::point(current.x, if settled { target } else { next }));
            self.last_follow_offset = Some(self.scroll.offset().y);
            self.last_follow_max = self.scroll.max_offset().y;
            if !settled || content != self.last_content {
                let owner = cx.entity().downgrade();
                let pending = self.motion_frame_pending.clone();
                if !pending.replace(true) {
                    cx.spawn(async move |_, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(16))
                            .await;
                        pending.set(false);
                        let _ = owner.update(cx, |_, cx| cx.notify());
                    })
                    .detach();
                }
            }
        }
        self.last_content = content;
        // Only the empty bootstrap gets a connection interstitial. Existing work,
        // actionable requests, and failures keep their transcript and recovery UI.
        if state.status == codex::Status::Starting
            && state.saved.items.is_empty()
            && state.questions.is_empty()
            && self.question_receipts.is_empty()
            && state.approvals.is_empty()
            && state.elicitations.is_empty()
            && state.error.is_none()
        {
            return div()
                .id("codex-conversation")
                .size_full()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .px(rpx(SPACE_20))
                .font_family(crate::fonts::UI_FAMILY)
                .text_size(rpx(TEXT_BODY))
                .text_color(c::FG_DIM())
                .child(
                    div()
                        .id("codex-provider-connection")
                        .debug_selector(|| "codex-provider-connection".into())
                        .child(shimmer_label(
                            "codex-provider-connection-shimmer".into(),
                            format!("Connecting to {provider}…"),
                            true,
                            cx,
                        )),
                )
                .into_any_element();
        }
        let minimap_entries = message_minimap_entries(&state.saved.items);
        let mut minimap_anchors = Vec::new();
        let mut message_child = 0;
        let mut messages = div()
            .id("codex-messages")
            .debug_selector(|| "codex-messages".into())
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                let delta_y = match event.delta {
                    gpui::ScrollDelta::Lines(delta) => delta.y,
                    gpui::ScrollDelta::Pixels(delta) => f32::from(delta.y),
                };
                if delta_y != 0.0 {
                    this.transcript_scrolled(delta_y < 0.0, cx);
                }
            }))
            .flex()
            .flex_col()
            .items_center()
            .px(rpx(SPACE_20))
            .py(rpx(SPACE_3XL))
            .gap(rpx(SPACE_20));
        let focused_launch = state.saved.items.is_empty()
            && state.questions.is_empty()
            && self.question_receipts.is_empty()
            && state.approvals.is_empty()
            && state.elicitations.is_empty()
            && !state.status.busy();
        for (index, item) in state
            .saved
            .items
            .iter()
            .chain(std::iter::once(&serde_json::Value::Null))
            .enumerate()
        {
            if index < state.saved.items.len() {
                let blocks = self.question_blocks_at(index, &state, window, cx);
                message_child += blocks.len();
                messages = messages.children(blocks);
            }
            if let Some((key, label, active)) = work_header_at(&state, index) {
                message_child += 1;
                let expanded = self.expanded.contains(&key);
                let keyboard_key = key.clone();
                messages = messages.child(
                    div()
                        .w_full()
                        .max_w(rpx(CHAT_MAX_W))
                        .pb(rpx(SPACE_XS))
                        .border_b_1()
                        .border_color(c::BORDER_SOFT())
                        .child(
                            header_control("codex-work-header", "Show work activity")
                                .id(("codex-work-header", index))
                                .aria_expanded(expanded)
                                .debug_selector(move || format!("codex-work-header-{index}"))
                                .w_full()
                                .justify_start()
                                .gap(rpx(SPACE_SM))
                                .tab_index(0)
                                .focus_visible(|style| style.bg(c::BG_HOVER()))
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .aria_label(label.clone())
                                .child(shimmer_label(
                                    ("codex-work-shimmer", index).into(),
                                    label,
                                    false,
                                    cx,
                                ))
                                .child(crate::icons::icon(
                                    if expanded { "chev-down" } else { "chev-right" },
                                    ICON_SM,
                                    c::FG_DIM(),
                                ))
                                .on_key_down(cx.listener(
                                    move |this, event: &gpui::KeyDownEvent, _, cx| {
                                        if action_key(event) {
                                            this.toggle_expansion(&keyboard_key, cx);
                                            cx.stop_propagation();
                                        }
                                    },
                                ))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_expansion(&key, cx);
                                })),
                        ),
                );
                if active && state.work_started.is_some() && !self.work_tick_pending.replace(true) {
                    let owner = cx.entity().downgrade();
                    let pending = self.work_tick_pending.clone();
                    cx.spawn(async move |_, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_secs(1))
                            .await;
                        pending.set(false);
                        let _ = owner.update(cx, |_, cx| cx.notify());
                    })
                    .detach();
                }
            }
            if index == state.saved.items.len() {
                continue;
            }
            let kind = item["type"].as_str().unwrap_or("tool");
            // Keep completed commentary with the work it describes; the answer stays inline.
            if work_commentary(&state, index)
                && !self.expanded.contains(&work_key_at(&state, index))
                && !work_is_active_at(&state, index)
            {
                continue;
            }
            if kind == codex::COMPACTION_ITEM {
                // A running compaction is shown by the live activity line instead.
                if item["status"] != "inProgress" {
                    message_child += 1;
                    messages = messages.child(compaction_divider(index, codex::item_text(item)));
                }
                continue;
            }
            let tool = is_tool_item(item);
            if tool {
                if index > 0
                    && is_tool_item(&state.saved.items[index - 1])
                    && (state.saved.items[index - 1]["type"] == "reasoning")
                        == (kind == "reasoning")
                    && !self.has_question_anchor(index)
                {
                    continue;
                }
                let end = state.saved.items[index..]
                    .iter()
                    .position(|item| {
                        !is_tool_item(item)
                            || (item["type"] == "reasoning") != (kind == "reasoning")
                    })
                    .map_or(state.saved.items.len(), |offset| index + offset);
                let end = (index + 1..end)
                    .find(|anchor| self.has_question_anchor(*anchor))
                    .unwrap_or(end);
                let tools = state.saved.items[index..end]
                    .iter()
                    .enumerate()
                    .filter(|(offset, _)| {
                        let item_index = index + offset;
                        tool_visible(&state, item_index)
                            && (work_is_active_at(&state, item_index)
                                || self.expanded.contains(&work_key_at(&state, item_index)))
                    })
                    .map(|(offset, item)| (index + offset, item))
                    .collect::<Vec<_>>();
                if tools.is_empty() {
                    continue;
                }
                let group_id = format!("tools-{}", item["id"].as_str().unwrap_or_default());
                let group_id = if item["id"].as_str().is_some() {
                    group_id
                } else {
                    format!("tools-{index}")
                };
                let expanded = !self.expanded.contains(&group_id);
                let running = tools
                    .iter()
                    .any(|(index, _)| tool_is_running(&state, *index));
                let label = if tools
                    .iter()
                    .all(|(_, item)| item["type"] == "commandExecution")
                {
                    if running {
                        "Running commands"
                    } else {
                        "Ran commands"
                    }
                } else if running {
                    "Working"
                } else {
                    "Activity"
                }
                .to_owned();
                let summary = div()
                    .id(("codex-tool-summary", index))
                    .debug_selector(move || format!("codex-tool-summary-{index}"))
                    .relative()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_SM))
                    .child(crate::icons::icon("terminal", ICON_SM, c::FG_DIM()))
                    .child(
                        div()
                            .id(("codex-tool-label", index))
                            .debug_selector(move || format!("codex-tool-label-{index}"))
                            .min_w_0()
                            .truncate()
                            .child(tool_activity_label(label.clone(), index, false, cx)),
                    )
                    .child(
                        div()
                            .id(("codex-tool-chevron", index))
                            .debug_selector(move || format!("codex-tool-chevron-{index}"))
                            .size(rpx(ICON_SM))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .child(crate::icons::icon(
                                if expanded { "chev-down" } else { "chev-right" },
                                ICON_SM,
                                c::FG_DIM(),
                            )),
                    );
                let mut group = div()
                    .id(("codex-tool-group", index))
                    .debug_selector(move || format!("codex-tool-group-{index}"))
                    // Adjacent activity groups share the compact rhythm of their rows;
                    // prose keeps the transcript's normal paragraph separation.
                    .when(
                        state.saved.items.get(end).is_some_and(is_tool_item)
                            && tool_visible(&state, end)
                            && (work_is_active_at(&state, end)
                                || self.expanded.contains(&work_key_at(&state, end))),
                        |group| group.mb(-rpx(SPACE_2XL)),
                    )
                    .w_full()
                    .max_w(rpx(CHAT_MAX_W))
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_XS));
                let keyboard_group = group_id.clone();
                if tools.len() > 1 {
                    group = group.child(
                        header_control(
                            "codex-toggle-tools",
                            if expanded {
                                "Collapse tool calls"
                            } else {
                                "Expand tool calls"
                            },
                        )
                        .id(("codex-toggle-tools", index))
                        .debug_selector(move || format!("codex-toggle-tools-{index}"))
                        .aria_expanded(expanded)
                        .w_full()
                        .justify_start()
                        .tab_index(0)
                        .focus_visible(|style| style.bg(c::BG_HOVER()))
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .aria_label(label)
                        .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space")
                                && !event.is_held
                            {
                                if !this.expanded.remove(&keyboard_group) {
                                    this.expanded.insert(keyboard_group.clone());
                                }
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }))
                        .child(summary)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.expanded.remove(&group_id) {
                                this.expanded.insert(group_id.clone());
                            }
                            cx.notify();
                        })),
                    );
                }
                if expanded || tools.len() == 1 {
                    for (detail_index, tool) in tools {
                        let tool_key = tool_expansion_key(tool, detail_index);
                        let tool_expanded = self.expanded.contains(&tool_key);
                        let keyboard_key = tool_key.clone();
                        let mut detail_row = div()
                            .id(("codex-tool-detail-row", detail_index))
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_SM))
                            .child(
                                header_control(
                                    "codex-toggle-tool",
                                    if tool_expanded {
                                        "Collapse tool call"
                                    } else {
                                        "Expand tool call"
                                    },
                                )
                                .id(("codex-toggle-tool", detail_index))
                                .debug_selector(move || format!("codex-toggle-tool-{detail_index}"))
                                .w_full()
                                .justify_start()
                                .gap(rpx(SPACE_SM))
                                .tab_index(0)
                                .focus_visible(|style| style.bg(c::BG_HOVER()))
                                .aria_label(tool_activity_summary(
                                    tool,
                                    tool_is_running(&state, detail_index),
                                ))
                                .child(
                                    div()
                                        .id(("codex-tool-icon", detail_index))
                                        .debug_selector(move || {
                                            format!("codex-tool-icon-{detail_index}")
                                        })
                                        .size(rpx(ICON_SM))
                                        .flex_shrink_0()
                                        .child(crate::icons::icon(
                                            if tool["type"] == "reasoning" {
                                                "brain"
                                            } else {
                                                "terminal"
                                            },
                                            ICON_SM,
                                            c::FG_DIM(),
                                        )),
                                )
                                .child(
                                    div()
                                        .id(("codex-tool-row-label", detail_index))
                                        .debug_selector(move || {
                                            format!("codex-tool-row-label-{detail_index}")
                                        })
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(rpx(TEXT_SMALL))
                                        .text_color(c::FG_DIM())
                                        .child(shimmer_label(
                                            ("codex-tool-detail-shimmer", detail_index).into(),
                                            tool_activity_summary(
                                                tool,
                                                tool_is_running(&state, detail_index),
                                            ),
                                            work_is_active_at(&state, detail_index)
                                                && tool_visible(&state, detail_index),
                                            cx,
                                        )),
                                )
                                .aria_expanded(tool_expanded)
                                .child(crate::icons::icon(
                                    if tool_expanded {
                                        "chev-down"
                                    } else {
                                        "chev-right"
                                    },
                                    ICON_SM,
                                    c::FG_DIM(),
                                ))
                                .on_key_down(cx.listener(
                                    move |this, event: &gpui::KeyDownEvent, _, cx| {
                                        if matches!(event.keystroke.key.as_str(), "enter" | "space")
                                            && !event.is_held
                                        {
                                            this.toggle_expansion(&keyboard_key, cx);
                                            cx.stop_propagation();
                                        }
                                    },
                                ))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| this.toggle_expansion(&tool_key, cx),
                                )),
                            );
                        if tool_expanded {
                            let copy = tool_detail(tool);
                            let keyboard_copy = copy.clone();
                            detail_row = detail_row.child(
                                div()
                                    .id(("codex-tool-detail", detail_index))
                                    .debug_selector(move || {
                                        format!("codex-tool-detail-{detail_index}")
                                    })
                                    .max_h(rpx(TOOL_DETAIL_MAX_H))
                                    .overflow_y_scroll()
                                    .p(rpx(SPACE_LG))
                                    .rounded(rpx(RADIUS_CONTROL))
                                    .bg(c::BG_RAIL())
                                    .flex()
                                    .flex_col()
                                    .gap(rpx(SPACE_SM))
                                    .child(
                                        header_control("codex-copy-tool", "Copy tool call")
                                            .tab_index(0)
                                            .focus_visible(|style| style.bg(c::BG_HOVER()))
                                            .child(crate::icons::icon("copy", ICON_MD, c::FG_DIM()))
                                            .on_key_down(move |event, _, cx| {
                                                if matches!(
                                                    event.keystroke.key.as_str(),
                                                    "enter" | "space"
                                                ) && !event.is_held
                                                {
                                                    cx.write_to_clipboard(
                                                        gpui::ClipboardItem::new_string(
                                                            keyboard_copy.clone(),
                                                        ),
                                                    );
                                                    cx.stop_propagation();
                                                }
                                            })
                                            .on_click(move |_, _, cx| {
                                                cx.write_to_clipboard(
                                                    gpui::ClipboardItem::new_string(copy.clone()),
                                                );
                                            }),
                                    )
                                    .child(
                                        TextView::markdown(
                                            ("codex-group-tool-text", detail_index),
                                            format!(
                                                "```text\n{}\n```",
                                                tool_detail(tool).replace("```", "` ` `")
                                            ),
                                        )
                                        .style(markdown_style(window))
                                        .selectable(true),
                                    ),
                            );
                        }
                        group = group.child(detail_row);
                    }
                }
                message_child += 1;
                messages = messages.child(group);
                continue;
            }
            let id = item["id"]
                .as_str()
                .map_or_else(|| index.to_string(), str::to_owned);
            let expanded = self.expanded.contains(&id);
            let text = codex::item_text(item);
            let role = match kind {
                "userMessage" => "You",
                "agentMessage" => provider,
                "plan" => "Plan",
                "commandExecution" => "Command",
                "fileChange" => "File changes",
                "reasoning" => "Reasoning",
                _ => "Tool",
            };
            let mut row = div()
                .id(("codex-item", index))
                .debug_selector(move || format!("codex-message-row-{index}"))
                .flex()
                .flex_col()
                .w_full()
                .max_w(rpx(if kind == "userMessage" {
                    USER_BUBBLE_MAX_W
                } else {
                    CHAT_MAX_W
                }))
                .gap(rpx(SPACE_LG))
                .when(kind == "userMessage", |row| {
                    row.p(rpx(SPACE_2XL))
                        .rounded(rpx(RADIUS_PANEL))
                        .bg(c::BG_RAIL())
                });
            let text = if tool { tool_detail(item) } else { text };
            let copy = text.clone();
            let text = if kind == "agentMessage" && !cx.reduce_motion() {
                let key = item["id"]
                    .as_str()
                    .map_or_else(|| format!("message-{index}"), str::to_owned);
                let entry = self.stream_text.entry(key).or_insert_with(|| {
                    let shown = if state.status.busy()
                        && !initial_transcript
                        && index + 1 == state.saved.items.len()
                    {
                        0
                    } else {
                        text.len()
                    };
                    (text.clone(), shown)
                });
                if !text.starts_with(&entry.0[..entry.1.min(entry.0.len())]) {
                    entry.1 = text.len();
                }
                entry.0.clone_from(&text);
                if !state.status.busy() {
                    entry.1 = text.len();
                }
                if entry.1 < text.len() {
                    self.smooth_follow |= near_bottom && !self.composer_collapsed;
                    // Catch up over a short interval without replaying the existing prefix.
                    let remaining = text[entry.1..].chars().count();
                    let advance = ((remaining as f32
                        * (elapsed.min(1.0 / 30.0) / (MOTION_FAST_MS as f32 / 1000.0)).min(1.0))
                    .ceil() as usize)
                        .max(1);
                    entry.1 = text[entry.1..]
                        .char_indices()
                        .nth(advance)
                        .map_or(text.len(), |(offset, _)| entry.1 + offset);
                    let owner = cx.entity().downgrade();
                    let pending = self.motion_frame_pending.clone();
                    if !pending.replace(true) {
                        cx.spawn(async move |_, cx| {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(16))
                                .await;
                            pending.set(false);
                            let _ = owner.update(cx, |_, cx| cx.notify());
                        })
                        .detach();
                    }
                }
                text[..entry.1].to_owned()
            } else {
                if kind == "agentMessage" {
                    let key = item["id"]
                        .as_str()
                        .map_or_else(|| format!("message-{index}"), str::to_owned);
                    self.stream_text.insert(key, (text.clone(), text.len()));
                }
                text
            };
            let toggle = id.clone();
            if kind != "agentMessage" {
                row = row.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_LG))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child(if tool {
                                    tool_summary(item)
                                } else {
                                    role.to_owned()
                                }),
                        )
                        .when(tool, |row| {
                            row.child(
                                header_control(
                                    "codex-expand",
                                    if expanded {
                                        "Collapse tool call"
                                    } else {
                                        "Expand tool call"
                                    },
                                )
                                .child(crate::icons::icon(
                                    if expanded { "chev-down" } else { "chev-right" },
                                    ICON_MD,
                                    c::FG_DIM(),
                                ))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if !this.expanded.remove(&toggle) {
                                            this.expanded.insert(toggle.clone());
                                        }
                                        cx.notify();
                                    },
                                )),
                            )
                        })
                        .child(
                            header_control("codex-copy", "Copy message")
                                .child(crate::icons::icon("copy", ICON_MD, c::FG_DIM()))
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        copy.clone(),
                                    ));
                                }),
                        ),
                );
            }
            if !tool {
                row = row.child(
                    div()
                        .id(format!("codex-message-body-{index}"))
                        .debug_selector(move || format!("codex-message-body-{index}"))
                        .child(
                            TextView::markdown(("codex-text", index), text)
                                .markdown_extensions(super::mermaid::extensions())
                                .style(markdown_style(window))
                                .selectable(true),
                        ),
                );
                if kind == "userMessage" {
                    if let Some(content) = item["content"].as_array() {
                        let images: Vec<_> = content
                            .iter()
                            .filter_map(|part| {
                                part["url"].as_str().or_else(|| part["image_url"].as_str())
                            })
                            .collect();
                        if !images.is_empty() {
                            let count = images.len();
                            let label =
                                format!("{count} attachment{}", if count == 1 { "" } else { "s" });
                            let mut thumbnails = div()
                                .id(("codex-message-images", index))
                                .debug_selector(move || format!("codex-message-images-{index}"))
                                .flex()
                                .flex_wrap()
                                .gap(rpx(SPACE_LG));
                            for (image_index, image) in images.into_iter().enumerate() {
                                if let Some(image) =
                                    self.previews.get(image).cloned().or_else(|| {
                                        let preview = image_from_url(image)?;
                                        self.previews.insert(image.to_owned(), preview.clone());
                                        Some(preview)
                                    })
                                {
                                    thumbnails = thumbnails.child(
                                        attachment_preview(image)
                                            .id(format!(
                                                "codex-message-image-{index}-{image_index}"
                                            ))
                                            .debug_selector(move || {
                                                format!("codex-message-image-{index}-{image_index}")
                                            }),
                                    );
                                }
                            }
                            row = row.child(
                                div()
                                    .id(("codex-message-attachments", index))
                                    .debug_selector(move || {
                                        format!("codex-message-attachments-{index}")
                                    })
                                    .flex()
                                    .flex_col()
                                    .gap(rpx(SPACE_LG))
                                    .child(
                                        div()
                                            .id(("codex-message-attachment-count", index))
                                            .debug_selector(move || {
                                                format!("codex-message-attachment-count-{index}")
                                            })
                                            .text_size(rpx(TEXT_SMALL))
                                            .text_color(c::FG_DIM())
                                            .child(label),
                                    )
                                    .child(thumbnails),
                            );
                        }
                    }
                }
            } else if expanded {
                row = row.child(
                    div()
                        .id(("codex-tool-detail", index))
                        .debug_selector(move || format!("codex-tool-detail-{index}"))
                        .child(
                            TextView::markdown(
                                ("codex-tool-text", index),
                                format!("```text\n{}\n```", text.replace("```", "` ` `")),
                            )
                            .style(markdown_style(window))
                            .selectable(true),
                        ),
                );
            }
            let row = if let Some((started, finished)) = self.animating_bubbles.get(&id) {
                let started = *started;
                let executor = cx.background_executor().clone();
                let finished = finished.clone();
                row.relative()
                    .with_animation(
                        gpui::SharedString::from(format!("codex-send-{id}")),
                        gpui::Animation::new(std::time::Duration::from_millis(MOTION_SLOW_MS))
                            .with_easing(super::motion::ease_standard),
                        move |row, _progress| {
                            // One executor clock preserves progress across temporary unmounts
                            // and keeps virtual-time tests independent of wall-clock delays.
                            let elapsed = executor.now().duration_since(started).as_secs_f32()
                                / std::time::Duration::from_millis(MOTION_SLOW_MS).as_secs_f32();
                            let progress = super::motion::ease_standard(elapsed.min(1.0));
                            finished.set(elapsed >= 1.0);
                            // Rise from the composer, then settle with a small overshoot.
                            let remaining = 1.0 - progress;
                            row.top(rpx(SPACE_3XL
                                * remaining
                                * remaining
                                * (1.0 - 2.0 * progress)))
                        },
                    )
                    .into_any_element()
            } else {
                row.into_any_element()
            };
            if kind == "userMessage" {
                minimap_anchors.push((index, message_child));
            }
            message_child += 1;
            messages = messages.child(
                div()
                    .id(("codex-message-anchor", index))
                    .debug_selector(move || format!("codex-message-anchor-{index}"))
                    .w_full()
                    .max_w(rpx(CHAT_MAX_W))
                    .flex()
                    .when(kind == "userMessage", gpui::Styled::justify_end)
                    .child(row),
            );
        }
        messages =
            messages.children(self.question_blocks_at(state.saved.items.len(), &state, window, cx));
        let minimap = self.render_message_minimap(&minimap_entries, &minimap_anchors, window, cx);
        let transcript = div()
            .id("codex-transcript")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .child(minimap)
            .child(messages);
        let mut surface = div()
            .id("codex-conversation")
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .font_family(crate::fonts::UI_FAMILY)
            .text_size(rpx(TEXT_BODY))
            .text_color(c::FG())
            .when(!focused_launch, |surface| surface.child(transcript))
            .when(
                focused_launch,
                gpui::StatefulInteractiveElement::overflow_y_scroll,
            )
            // Cmd+Enter resolves to the shell's global Zen action before Input's
            // secondary-enter binding. Own it only while the main draft has focus.
            .on_action(
                cx.listener(|this, _: &crate::keymap::ToggleZen, window, cx| {
                    if this.input.focus_handle(cx).is_focused(window) {
                        if this.session.read(cx).state.status.busy() {
                            this.submit_mode(SubmissionMode::Steer, window, cx);
                        } else {
                            this.submit(window, cx);
                        }
                    } else {
                        cx.propagate();
                    }
                }),
            )
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                if this.input.focus_handle(cx).is_focused(window)
                    && this.accept_first_suggestion(window, cx)
                {
                    cx.stop_propagation();
                } else {
                    cx.propagate();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
                if !this.move_suggestion(false, window, cx)
                    && !this.move_queue_selection(false, window, cx)
                {
                    cx.propagate();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveUp, window, cx| {
                if !this.move_suggestion(true, window, cx)
                    && !this.move_queue_selection(true, window, cx)
                {
                    cx.propagate();
                }
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && this.queue_editor.is_some() {
                    this.cancel_queue_edit(window, cx);
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape" && this.queue_selected.take().is_some() {
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape"
                    && this.popup.is_none()
                    && trailing_trigger(&this.input.read(cx).value()).is_some()
                {
                    this.menu = None;
                    this.dismissed_suggestions = Some(this.input.read(cx).value().to_string());
                    this.input.focus_handle(cx).focus(window, cx);
                    cx.notify();
                    cx.stop_propagation();
                }
            }));
        if let Some(activity) = live_activity(&state) {
            let running = compacting(&state)
                || (state.status.busy()
                    && state.questions.is_empty()
                    && state.elicitations.is_empty()
                    && state.approvals.is_empty());
            surface =
                surface.child(
                    div()
                        .id("codex-live-activity")
                        .debug_selector(|| "codex-live-activity".into())
                        .w_full()
                        .flex_shrink_0()
                        .flex()
                        .justify_center()
                        .px(rpx(SPACE_20))
                        .py(rpx(SPACE_SM))
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(div().w_full().max_w(rpx(CHAT_MAX_W)).truncate().child(
                            shimmer_label("codex-activity-shimmer".into(), activity, running, cx),
                        )),
                );
        }
        if let Some(error) = &state.error {
            surface = surface.child(content_rail(
                div()
                    .id("codex-error")
                    .debug_selector(|| "codex-error".into())
                    .role(gpui::Role::Alert)
                    .py(rpx(SPACE_SM))
                    .text_color(c::RED())
                    .child(error.clone()),
            ));
        }
        if !state.approvals.is_empty() || !state.elicitations.is_empty() {
            let approvals = self.approval_panel(&state, window, cx);
            let elicitations = self.elicitation_panel(&state, window, cx);
            surface = surface.child(
                div()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .max_h(rpx(QUESTION_PANEL_MAX_H))
                    .flex()
                    .justify_center()
                    .px(rpx(SPACE_20))
                    .child(
                        div()
                            .id("agent-controls")
                            .debug_selector(|| "agent-controls".into())
                            .w_full()
                            .min_w_0()
                            .h_full()
                            .min_h_0()
                            .max_w(rpx(CHAT_MAX_W))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_3XL))
                            .py(rpx(SPACE_LG))
                            .child(approvals)
                            .child(elicitations),
                    ),
            );
        }
        self.previews.retain(|url, _| {
            self.images.contains(url)
                || state.saved.items.iter().any(|item| {
                    item["content"].as_array().is_some_and(|content| {
                        content.iter().any(|part| {
                            part["url"].as_str() == Some(url)
                                || part["image_url"].as_str() == Some(url)
                        })
                    })
                })
        });
        let queued = self.queue_tray(&state, window, cx);
        let mut attachments = div()
            .id("codex-attachments")
            .debug_selector(|| "codex-attachments".into())
            .flex_shrink_0()
            .flex()
            .flex_wrap()
            .gap(rpx(SPACE_LG));
        for (index, url) in self.images.iter().enumerate() {
            if let Some(image) = self.previews.get(url).cloned() {
                attachments = attachments.child(
                    attachment_preview(image)
                        .id(("codex-image", index))
                        .debug_selector(move || format!("codex-image-{index}"))
                        .group("codex-image")
                        .child(
                            header_control("codex-remove-image", "Remove attached image")
                                .tab_index(0)
                                .on_key_down(cx.listener(
                                    move |this, event: &gpui::KeyDownEvent, window, cx| {
                                        if matches!(event.keystroke.key.as_str(), "enter" | "space")
                                            && this.submitted.is_none()
                                            && !event.is_held
                                        {
                                            this.remove_image(index, window, cx);
                                            cx.stop_propagation();
                                        }
                                    },
                                ))
                                .absolute()
                                .top_0()
                                .right_0()
                                .size(rpx(CHROME_CONTROL_H))
                                .p_0()
                                .bg(c::SURFACE_RAISED())
                                .opacity(0.0)
                                .group_hover("codex-image", |style| style.opacity(1.0))
                                .focus_visible(|style| style.opacity(1.0))
                                .child(crate::icons::icon("close", ICON_MD, c::FG_DIM()))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if this.submitted.is_some() {
                                        return;
                                    }
                                    this.remove_image(index, window, cx);
                                })),
                        ),
                );
            }
        }
        if !state.status.busy()
            || self.interrupt_requested_for_turn != state.turn_id
            || state.error.is_some()
        {
            self.interrupt_requested_for_turn = None;
        }
        let busy = state.status.busy();
        let stopping = busy && self.interrupt_requested_for_turn.is_some();
        let draft_disabled = matches!(
            state.status,
            codex::Status::Starting | codex::Status::Disconnected
        ) || self.submitted.is_some()
            || self.pending_image_pastes > 0
            || stopping
            || (self.input.read(cx).value().trim().is_empty() && self.images.is_empty());
        let steer_available =
            state.saved.provider == codex::Provider::Codex && state.turn_id.is_some();
        let editing = self
            .queue_editor
            .as_ref()
            .is_some_and(|editor| editor.loaded);
        let primary_disabled = if editing {
            self.queue_editor
                .as_ref()
                .is_some_and(|editor| editor.saving)
                || (self.input.read(cx).value().trim().is_empty() && self.images.is_empty())
        } else {
            draft_disabled
        };
        let primary_label = if editing {
            "Save queued message · Enter"
        } else if busy {
            "Queue · Enter · Send after the current turn finishes"
        } else {
            "Send · Enter · Start a new turn"
        };
        let choices = self.composer_choices(&state, window, cx);
        let collapsed = self.composer_collapsed;
        let target = self.composer_body_target.clone();
        let expanded_height = self.composer_expanded_height.clone();
        if self.composer_transition_from.is_some() {
            target.set(if collapsed {
                rpx(CHROME_CONTROL_H).to_pixels(window.rem_size())
            } else {
                expanded_height
                    .get()
                    .max(rpx(COMPOSER_MIN_H).to_pixels(window.rem_size()))
            });
        }
        let owner = cx.entity().downgrade();
        let body_height = self.composer_body_height.clone();
        let height_owner = cx.entity().downgrade();
        let body = div()
            .relative()
            .w_full()
            .min_h_0()
            .flex_shrink_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                gpui::canvas(
                    move |bounds, _, cx| {
                        let changed = body_height.replace(bounds.size.height) != bounds.size.height;
                        let _ = height_owner.update(cx, |this, cx| {
                            if changed && this.composer_pin_bottom {
                                this.smooth_follow = true;
                                cx.notify();
                            }
                            if this.composer_motion_finished.get() {
                                this.composer_pin_bottom = false;
                            }
                        });
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .relative()
                    .w_full()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_LG))
                    .child(
                        gpui::canvas(
                            move |bounds, _, cx| {
                                let changed =
                                    target.replace(bounds.size.height) != bounds.size.height;
                                if !collapsed {
                                    expanded_height.set(bounds.size.height);
                                }
                                if changed {
                                    let _ = owner.update(cx, |_, cx| cx.notify());
                                }
                            },
                            |_, (), _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .when(!collapsed && !self.images.is_empty(), |body| {
                        body.child(attachments.mb(rpx(SPACE_LG)))
                    })
                    .child(
                        div()
                            .id("codex-composer-input")
                            .debug_selector(|| "codex-composer-input".into())
                            .relative()
                            .min_h(rpx(if self.composer_collapsed {
                                CHROME_CONTROL_H
                            } else {
                                COMPOSER_MIN_H
                            }))
                            .capture_any_mouse_down(cx.listener(
                                |this, _: &gpui::MouseDownEvent, _, cx| {
                                    this.set_composer_collapsed(false, cx);
                                },
                            ))
                            .child(
                                Input::new(&self.input)
                                    .disabled(
                                        self.submitted.is_some()
                                            || self.queue_editor.as_ref().is_some_and(|editor| {
                                                !editor.loaded || editor.saving
                                            }),
                                    )
                                    .aria_label(format!("Message {provider}"))
                                    .appearance(false)
                                    .bordered(false)
                                    .focus_bordered(false)
                                    .text_color(c::FG())
                                    .p_0(),
                            )
                            .when(self.composer_collapsed, |field| {
                                field.child(
                                    div()
                                        .id("codex-composer-preview")
                                        .debug_selector(|| "codex-composer-preview".into())
                                        .role(gpui::Role::Button)
                                        .aria_label("Expand message input")
                                        .absolute()
                                        .inset_0()
                                        .flex()
                                        .items_center()
                                        .bg(c::BG_RAIL())
                                        .child(
                                            div()
                                                .w_full()
                                                .min_w_0()
                                                .truncate()
                                                .text_color(
                                                    if self.input.read(cx).value().is_empty() {
                                                        c::FG_DIM()
                                                    } else {
                                                        c::FG()
                                                    },
                                                )
                                                .child(if self.input.read(cx).value().is_empty() {
                                                    composer_placeholder(state.saved.provider)
                                                        .to_owned()
                                                } else {
                                                    collapsed_draft_preview(
                                                        &self.input.read(cx).value(),
                                                    )
                                                }),
                                        ),
                                )
                            }),
                    ),
            );
        let body = if let Some(from) = self
            .composer_transition_from
            .filter(|_| !cx.reduce_motion())
        {
            let target = self.composer_body_target.clone();
            let finished = self.composer_motion_finished.clone();
            body.with_animation(
                ("codex-composer-height", self.composer_transition_generation),
                gpui::Animation::new(std::time::Duration::from_millis(MOTION_BASE_MS))
                    .with_easing(super::motion::ease_standard),
                move |body, progress| {
                    finished.set(progress >= 1.0);
                    body.h(from + (target.get() - from) * progress)
                },
            )
            .into_any_element()
        } else {
            body.into_any_element()
        };
        let surface =
            surface.child(
                div()
                    .flex_shrink_0()
                    .w_full()
                    .when(focused_launch, |rail| rail.my_auto().pt(rpx(SPACE_3XL)))
                    .px(rpx(SPACE_20))
                    .pb(rpx(SPACE_3XL))
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .w_full()
                            .max_w(rpx(CHAT_MAX_W))
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_MD))
                            .when(focused_launch, |rail| rail.child(self.launch_intro(&state)))
                            .when(!self.composer_collapsed, |rail| rail.child(choices))
                            .when(!self.queue_motion_rows.is_empty(), |rail| rail.child(queued))
                            .child(
                        div()
                            .id("codex-composer")
                            .debug_selector(|| "codex-composer".into())
                            .w_full()
                            .max_w(rpx(CHAT_MAX_W))
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_LG))
                            .px(rpx(SPACE_2XL))
                            .pt(rpx(SPACE_3XL))
                            .pb(rpx(SPACE_2XL))
                            .rounded(rpx(SPACE_20))
                            .border_1()
                            .border_color(c::BORDER_SOFT())
                            .bg(c::BG_RAIL())
                            .child(body)
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(rpx(SPACE_LG))
                                    .children(self.option_triggers(&state, cx))
                                    .child(div().flex_1())
                                    .when_some(
                                        state.saved.context.filter(|usage| usage.window > 0),
                                        |row, usage| row.child(context_meter(usage)),
                                    )
                                    .child(
                                        div().flex().items_center().gap(rpx(SPACE_LG)).flex_shrink_0().ml_auto()
                                            .child(
                                                composer_icon("codex-attach", "Attach files or images", "paperclip", self.submitted.is_some(), false)
                                                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                                                        if action_key(event) { this.attach_files(window, cx); cx.stop_propagation(); }
                                                    }))
                                                    .on_click(cx.listener(|this, _, window, cx| this.attach_files(window, cx)))
                                            )
                                            .when(!busy && state.error.is_some(), |row| row.child(
                                                form_action("codex-reconnect", "Reconnect", false, window, cx)
                                                    .debug_selector(|| "codex-reconnect".into())
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.session.update(cx, |session, cx| session.send(Action::Reconnect, cx));
                                                    }))
                                            ))
                                            .when(busy, |row| row.child(
                                                div().mr(rpx(SPACE_SM)).child(
                                                    composer_icon("codex-stop", if stopping { "Stopping the current turn" } else { "Stop · Interrupt the current turn" }, "stop", stopping || state.turn_id.is_none(), false)
                                                        .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                                                            if action_key(event) { this.interrupt(cx); cx.stop_propagation(); }
                                                        }))
                                                        .on_click(cx.listener(|this, _, _, cx| this.interrupt(cx)))
                                                )
                                            ).child(
                                                composer_icon("codex-steer", if steer_available { "Steer now · ⌘Enter · Send into the current turn" } else { "Steer unavailable" }, "arrow-up", draft_disabled || !steer_available, false)
                                                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                                                        if action_key(event) { this.submit_mode(SubmissionMode::Steer, window, cx); cx.stop_propagation(); }
                                                    }))
                                                    .on_click(cx.listener(|this, _, window, cx| this.submit_mode(SubmissionMode::Steer, window, cx)))
                                            ))
                                            .child(
                                                composer_icon("codex-send", primary_label, if busy && !editing { "queue" } else { "arrow-up" }, primary_disabled, true)
                                                    .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                                                        if action_key(event) {
                                                            if !primary_disabled { this.submit(window, cx); }
                                                            cx.stop_propagation();
                                                        }
                                                    }))
                                                    .on_click(cx.listener(move |this, _, window, cx| {
                                                        if !primary_disabled { this.submit(window, cx); }
                                                    }))
                                            )
                                    ),
                            )
                            )
                            .child(
                                div()
                                    .id("codex-composer-hints")
                                    .debug_selector(|| "codex-composer-hints".into())
                                    .w_full()
                                    .px(rpx(SPACE_2XL))
                                    .flex()
                                    .flex_wrap()
                                    .justify_end()
                                    .text_right()
                                    .gap(rpx(SPACE_SM))
                                    .text_size(rpx(TEXT_MICRO))
                                    .text_color(c::FG_DIM())
                                    .child(if stopping || self.submitted.is_some() {
                                        div().child(if stopping { "Stopping…" } else { "Sending…" })
                                    } else {
                                        div().flex().items_center().flex_wrap().gap(rpx(SPACE_LG))
                                            .child(div().flex().items_center().gap(rpx(SPACE_SM))
                                                .child(crate::icons::icon("enter", ICON_SM, c::FG_DIM()))
                                                .child(if editing { "Save" } else if busy { "Queue" } else { "Send" }))
                                            .when(busy, |row| row.child(div().flex().items_center().gap(rpx(SPACE_SM))
                                                .child(crate::icons::icon("command", ICON_SM, c::FG_DIM()))
                                                .child(crate::icons::icon("enter", ICON_SM, c::FG_DIM()))
                                                .child(if steer_available { "Steer now" } else { "Steer unavailable" })))
                                            .child(div().flex().items_center().gap(rpx(SPACE_SM))
                                                .child(crate::icons::icon("shift", ICON_SM, c::FG_DIM()))
                                                .child(crate::icons::icon("enter", ICON_SM, c::FG_DIM()))
                                                .child("New line"))
                                    })
                            )
                            .when(focused_launch, |rail| rail.child(self.launch_starters(provider, cx))),
                    ),
            );
        if let (Some(menu), Some(popup)) = (self.menu, self.popup.clone()) {
            let index = match menu {
                ComposerMenu::Model => 0,
                ComposerMenu::Effort => 1,
                ComposerMenu::Access => 2,
            };
            let bounds = self.option_bounds[index].get();
            let gap = rpx(SPACE_MD).to_pixels(window.rem_size());
            surface
                .child(gpui::deferred(
                    gpui::anchored()
                        .position_mode(gpui::AnchoredPositionMode::Window)
                        .anchor(gpui::Anchor::BottomLeft)
                        .position(gpui::point(bounds.left(), bounds.top() - gap))
                        .snap_to_window_with_margin(rpx(SPACE_LG).to_pixels(window.rem_size()))
                        .child(
                            div()
                                .id("codex-options-popup")
                                .debug_selector(|| "codex-options-popup".into())
                                .child(popup),
                        ),
                ))
                .into_any_element()
        } else {
            surface.into_any_element()
        }
    }
}
impl CodexView {
    fn close_menu(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        self.popup = None;
        self.popup_subscription = None;
        cx.notify();
    }
    fn open_menu(&mut self, menu: ComposerMenu, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.session.read(cx).state.clone();
        let (choices, _) = Self::menu_choices(&state, menu);
        let selected = match menu {
            ComposerMenu::Model => state.model.as_deref().or(state.resolved_model.as_deref()),
            ComposerMenu::Effort => state.effort.as_deref().or(state.resolved_effort.as_deref()),
            ComposerMenu::Access => state.access.as_deref().or(state.resolved_access.as_deref()),
        }
        .map(str::to_owned);
        let index = match menu {
            ComposerMenu::Model => 0,
            ComposerMenu::Effort => 1,
            ComposerMenu::Access => 2,
        };
        let context = self.option_focus[index].clone();
        let owner = cx.entity().downgrade();
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let width =
            COMPOSER_MENU_W.min(f32::from(window.viewport_size().width) / scale - SPACE_3XL);
        let height = COMPOSER_MENU_MAX_H
            .min(f32::from(window.viewport_size().height) / scale - APPBAR_H - SPACE_3XL);
        let popup = PopupMenu::build(window, cx, move |mut popup, window, _| {
            popup = popup
                .action_context(context)
                .scrollable(true)
                .max_w(rpx(width).to_pixels(window.rem_size()))
                .min_w(rpx(width).to_pixels(window.rem_size()))
                .max_h(rpx(height).to_pixels(window.rem_size()));
            for (value, label) in choices {
                let owner = owner.clone();
                let checked = selected.as_deref() == Some(value.as_str());
                popup = popup.item(PopupMenuItem::new(label).checked(checked).on_click(
                    move |_, window, cx| {
                        let _ = owner.update(cx, |this, cx| this.choose_option(&value, window, cx));
                    },
                ));
            }
            popup
        });
        self.popup_subscription = Some(cx.subscribe_in(
            &popup,
            window,
            |this, _, _: &gpui::DismissEvent, _, cx| this.close_menu(cx),
        ));
        popup.focus_handle(cx).focus(window, cx);
        self.popup = Some(popup);
        self.menu = Some(menu);
        cx.notify();
    }
    fn option_triggers(
        &self,
        state: &codex::Snapshot,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let model = state
            .model
            .as_ref()
            .or(state.resolved_model.as_ref())
            .cloned()
            .unwrap_or_else(|| "Configured model".into());
        let model = state
            .models
            .iter()
            .find(|entry| {
                entry["model"].as_str() == Some(&model)
                    || (state.saved.provider == codex::Provider::Claude
                        && entry["resolvedModel"].as_str() == Some(&model))
            })
            .and_then(|entry| entry["displayName"].as_str())
            .unwrap_or(&model)
            .to_owned();
        let effort = state
            .effort
            .as_ref()
            .or(state.resolved_effort.as_ref())
            .cloned()
            .unwrap_or_else(|| "Default effort".into());
        let access_value = state.access.as_deref().or(state.resolved_access.as_deref());
        let access_icon = match access_value {
            Some("full-access" | "bypassPermissions") => "lock-open",
            Some("read-only" | "plan") => "search",
            Some("workspace-write" | "acceptEdits") => "edit",
            _ => "lock",
        };
        let access = match access_value {
            Some("full-access") => "Full access",
            Some("read-only") => "Read only",
            Some("workspace-write") => "Workspace access",
            Some("default") => "Ask for permission",
            Some("acceptEdits") => "Accept edits",
            Some("plan") => "Plan mode",
            Some("dontAsk") => "Deny unapproved tools",
            Some("bypassPermissions") => "Bypass permissions",
            _ => "Configured access",
        };
        let mut controls = Vec::new();
        for (index, (menu, id, name, icon, label)) in [
            (
                ComposerMenu::Model,
                "codex-model",
                "Choose model",
                state.saved.provider.agent().icon_name(),
                model,
            ),
            (
                ComposerMenu::Effort,
                "codex-effort",
                "Choose reasoning effort",
                "brain",
                effort,
            ),
            (
                ComposerMenu::Access,
                "codex-access",
                "Choose access level",
                access_icon,
                access.into(),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            if index > 0 {
                controls.push(
                    div()
                        .h(rpx(ICON_LG))
                        .border_l_1()
                        .border_color(c::BORDER_SOFT())
                        .into_any_element(),
                );
            }
            let bounds = self.option_bounds[index].clone();
            controls.push(
                header_control(id, name)
                    .relative()
                    .track_focus(&self.option_focus[index])
                    .child(
                        gpui::canvas(move |rect, _, _| bounds.set(rect), |_, (), _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .w_auto()
                    .min_w_0()
                    .px(rpx(SPACE_SM))
                    .gap(rpx(SPACE_MD))
                    .tab_index(0)
                    .text_color(c::FG_DIM())
                    .text_size(rpx(TEXT_SMALL))
                    .when(self.menu == Some(menu), |button| button.bg(c::BG_HOVER()))
                    .focus_visible(|style| style.bg(c::BG_HOVER()))
                    .child(crate::icons::icon(icon, ICON_SM, c::FG_DIM()))
                    .when(menu == ComposerMenu::Model, |button| {
                        button
                            .max_w_full()
                            .h_auto()
                            .min_h(rpx(CHROME_CONTROL_H))
                            .flex_wrap()
                    })
                    .child(
                        div()
                            .when(menu == ComposerMenu::Model, |label| {
                                label.max_w_full().flex_shrink_0()
                            })
                            .when(menu != ComposerMenu::Model, |label| {
                                label.max_w(rpx(FIELD_LABEL_COL_W)).truncate()
                            })
                            .child(label),
                    )
                    .child(crate::icons::icon("chev-down", ICON_SM, c::FG_DIM()))
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space" | "down")
                                && !event.is_held
                            {
                                this.open_menu(menu, window, cx);
                                cx.stop_propagation();
                            } else if event.keystroke.key == "escape" {
                                this.close_menu(cx);
                                cx.stop_propagation();
                            }
                        },
                    ))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.menu == Some(menu) {
                            this.close_menu(cx);
                        } else {
                            this.open_menu(menu, window, cx);
                        }
                    }))
                    .into_any_element(),
            );
        }
        controls
    }
    fn menu_choices(
        state: &codex::Snapshot,
        menu: ComposerMenu,
    ) -> (Vec<(String, String)>, &'static str) {
        let mut choices = Vec::new();
        let title = match menu {
            ComposerMenu::Model => {
                choices = state
                    .models
                    .iter()
                    .filter_map(|model| {
                        Some((
                            model["model"].as_str()?.to_owned(),
                            model["displayName"]
                                .as_str()
                                .unwrap_or(model["model"].as_str()?)
                                .to_owned(),
                        ))
                    })
                    .collect();
                "Model · applies to the next turn"
            }
            ComposerMenu::Effort => {
                if let Some(model) = effective_model(state) {
                    choices = model["supportedReasoningEfforts"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|entry| {
                            let effort = entry["reasoningEffort"].as_str()?;
                            Some((effort.to_owned(), effort.to_owned()))
                        })
                        .collect();
                }
                "Reasoning effort · applies to the next turn"
            }
            ComposerMenu::Access => {
                choices = if state.saved.provider == codex::Provider::Claude {
                    vec![
                        ("default", "Ask for permission · standard Claude checks"),
                        ("acceptEdits", "Accept edits · ask before other tools"),
                        ("plan", "Plan mode · explore before implementing"),
                        ("dontAsk", "Deny tools that require permission"),
                        (
                            "bypassPermissions",
                            "Bypass permissions · no approval prompts",
                        ),
                    ]
                } else {
                    vec![
                        ("read-only", "Read only · inspect without edits"),
                        (
                            "workspace-write",
                            "Workspace access · edit in this worktree",
                        ),
                        (
                            "full-access",
                            "Full access · no sandbox or approval prompts",
                        ),
                    ]
                }
                .into_iter()
                .map(|(value, label)| (value.into(), label.into()))
                .collect();
                "Access · applies only after your explicit selection"
            }
        };

        choices.push(("__configured".into(), "Restore configured defaults".into()));
        (choices, title)
    }
    fn suggestion_choices(
        &self,
        state: &codex::Snapshot,
        cx: &App,
    ) -> Option<(Vec<(String, String)>, &'static str)> {
        let input = self.input.read(cx);
        let text = input.value();
        if self.menu.is_some()
            || self.dismissed_suggestions.as_deref() == Some(text.as_ref())
            || !input.selected_range().is_empty()
        {
            return None;
        }
        let (_, trigger, query) = completion_trigger(&text, input.cursor())?;
        Some(match trigger {
            '@' => (
                self.file_suggestions
                    .iter()
                    .filter(|_| self.file_query == query)
                    .map(|path| (format!("@{path}"), path.clone()))
                    .collect(),
                "Files and folders · relative to this worktree",
            ),
            '$' => (
                state
                    .skills
                    .iter()
                    .filter_map(|skill| {
                        let name = skill["name"].as_str()?;
                        (name.to_lowercase().contains(&query.to_lowercase())
                            && (state.saved.provider != codex::Provider::Claude
                                || !host_command(name))
                            && skill["enabled"].as_bool() != Some(false))
                        .then(|| {
                            (
                                format!(
                                    "{}{name}",
                                    if state.saved.provider == codex::Provider::Claude {
                                        "/"
                                    } else {
                                        "$"
                                    }
                                ),
                                format!(
                                    "{name} · {}",
                                    skill["description"].as_str().unwrap_or("Skill")
                                ),
                            )
                        })
                    })
                    .take(8)
                    .collect(),
                "Skills",
            ),
            '/' => (slash_choices(state, query), "Commands"),
            _ => return None,
        })
    }

    fn move_suggestion(
        &mut self,
        previous: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.input.focus_handle(cx).is_focused(window) {
            return false;
        }
        let state = self.session.read(cx).state.clone();
        let Some((choices, _)) = self.suggestion_choices(&state, cx) else {
            return false;
        };
        if choices.is_empty() {
            return false;
        }
        self.suggestion_index = if previous {
            (self.suggestion_index + choices.len() - 1) % choices.len()
        } else {
            (self.suggestion_index + 1) % choices.len()
        };
        cx.notify();
        cx.stop_propagation();
        true
    }

    fn accept_first_suggestion(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.submitted.is_some() {
            return false;
        }
        let state = self.session.read(cx).state.clone();
        let Some((choices, _)) = self.suggestion_choices(&state, cx) else {
            return false;
        };
        let Some((value, _)) =
            choices.get(self.suggestion_index.min(choices.len().saturating_sub(1)))
        else {
            return false;
        };
        self.accept_suggestion(value, false, window, cx);
        true
    }

    fn validate_picked_tokens(&mut self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value();
        self.picked_tokens.retain(|token| {
            let ranges = token.decoration.get_ranges(cx);
            let valid = ranges.first().is_some_and(|range| {
                text.get(range.clone()) == Some(token.value.as_str())
                    && text[..range.start]
                        .chars()
                        .next_back()
                        .is_none_or(char::is_whitespace)
                    && text[range.end..]
                        .chars()
                        .next()
                        .is_none_or(char::is_whitespace)
            });
            if !valid {
                token.decoration.clear(cx);
            }
            valid
        });
    }

    fn accept_suggestion(
        &mut self,
        value: &str,
        execute: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = self.input.read(cx);
        let text = input.value();
        let Some((range, _, _)) = completion_trigger(&text, input.cursor()) else {
            return;
        };
        let start = range.start;
        let following_space = text[range.end..]
            .chars()
            .next()
            .filter(|character| character.is_whitespace());
        let suffix =
            if following_space.is_some() || (value.starts_with('@') && value.ends_with('/')) {
                ""
            } else {
                " "
            };
        self.input.update(cx, |input, cx| {
            input.set_selected_range(range.clone(), cx);
            input.replace(format!("{value}{suffix}"), window, cx);
            if let Some(character) = following_space {
                let caret = start + value.len() + character.len_utf8();
                input.set_selected_range(caret..caret, cx);
            }
        });
        self.validate_picked_tokens(cx);
        {
            let decoration = self.input.update(cx, |input, cx| {
                input.create_decorations_collection(
                    vec![TextDecoration::new(
                        start..start + value.len(),
                        gpui::HighlightStyle {
                            underline: Some(gpui::UnderlineStyle {
                                thickness: rpx(COMPOSER_TOKEN_UNDERLINE)
                                    .to_pixels(window.rem_size()),
                                color: Some(c::BORDER_STRONG()),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    )],
                    cx,
                )
            });
            self.picked_tokens.push(ComposerToken {
                value: value.into(),
                decoration,
            });
        }
        let replacement = self.input.read(cx).value().to_string();
        if self.queue_editor.is_none() {
            self.session.update(cx, |session, cx| {
                session.send(Action::Draft(replacement), cx);
            });
        }
        if execute && value.starts_with('/') {
            self.run_command(window, cx);
        } else if value.starts_with('@') {
            self.refresh_files(cx);
        }
        self.input.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn composer_choices(
        &self,
        state: &codex::Snapshot,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some((choices, title)) = self.suggestion_choices(state, cx) else {
            return div().into_any_element();
        };
        let choice_count = choices.len();
        let mut rows = div()
            .id("codex-composer-option-rows")
            .max_h(rpx(QUEUED_PROMPTS_MAX_H))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        if choices.is_empty() {
            rows = rows.child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_MUTE())
                    .child("No available choices. Check the connection or select a model first."),
            );
        }
        for (index, (value, label)) in choices.into_iter().enumerate() {
            let keyboard_value = value.clone();
            let (name, description) = label.split_once(" · ").map_or_else(
                || (label.clone(), None),
                |(name, description)| (name.to_string(), Some(description.to_string())),
            );
            rows = rows.child(
                header_control("codex-composer-option", "Select composer option")
                    .id(("codex-composer-option", index))
                    .aria_label(label.clone())
                    .aria_description("Tab to insert this option")
                    .when(index == self.suggestion_index, |row| row.bg(c::BG_HOVER()))
                    .debug_selector(move || format!("codex-composer-option-{index}"))
                    .w_full()
                    .h_auto()
                    .min_h(rpx(CONTROL_H))
                    .justify_start()
                    .gap(rpx(SPACE_LG))
                    .px(rpx(SPACE_LG))
                    .py(rpx(SPACE_SM))
                    .tab_index(0)
                    .focus_visible(|style| style.bg(c::BG_HOVER()))
                    .child(
                        div()
                            .min_w_0()
                            .max_w_full()
                            .flex_shrink_0()
                            .whitespace_normal()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG())
                            .child(name),
                    )
                    .when_some(description, |row, description| {
                        row.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child(description),
                        )
                    })
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space")
                                && !event.is_held
                            {
                                this.choose_option(&keyboard_value, window, cx);
                                cx.stop_propagation();
                            } else if event.keystroke.key == "escape" {
                                this.menu = None;
                                this.input.focus_handle(cx).focus(window, cx);
                                cx.notify();
                            }
                        },
                    ))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.choose_option(&value, window, cx);
                    })),
            );
        }
        composer_tray(window)
            .id("codex-composer-options")
            .debug_selector(|| "codex-composer-options".into())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .pb(rpx(SPACE_SM))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_MUTE())
                            .child(choice_count.to_string()),
                    ),
            )
            .child(rows)
            .into_any_element()
    }
    fn choose_option(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        if value == "__configured" {
            self.close_menu(cx);
            self.session.update(cx, |session, cx| {
                session.send(Action::ResetConfiguration, cx);
            });
        } else if let Some(menu) = self.menu.take() {
            self.popup = None;
            let action = Action::Configure {
                model: (menu == ComposerMenu::Model).then(|| value.to_owned()),
                effort: (menu == ComposerMenu::Effort).then(|| value.to_owned()),
                access: (menu == ComposerMenu::Access).then(|| value.to_owned()),
            };
            self.session
                .update(cx, |session, cx| session.send(action, cx));
        } else {
            self.accept_suggestion(value, true, window, cx);
        }
        if self.popup.is_none() {
            self.input.focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }
    fn run_command(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let text = self.input.read(cx).value().to_string();
        let mut parts = text.split_whitespace();
        let Some(command) = parts.next() else {
            return false;
        };
        if !command.strip_prefix('/').is_some_and(host_command) {
            return false;
        }
        let result = if parts.next().is_some() {
            Err(format!(
                "{command} does not accept arguments in Grove. Run {command} on its own."
            ))
        } else if self.pending_image_pastes > 0 {
            Err("Wait for attached images to finish loading before running a command.".into())
        } else {
            match command {
                "/model" | "/effort" | "/access" => {
                    self.open_menu(
                        match command {
                            "/model" => ComposerMenu::Model,
                            "/effort" => ComposerMenu::Effort,
                            _ => ComposerMenu::Access,
                        },
                        window,
                        cx,
                    );
                    Ok(())
                }
                "/stop" => {
                    let state = &self.session.read(cx).state;
                    if !state.status.busy() || state.turn_id.is_none() {
                        Err("There is no active turn to interrupt.".into())
                    } else if self.interrupt_requested_for_turn.as_ref() == state.turn_id.as_ref() {
                        Err("The current turn is already stopping.".into())
                    } else {
                        self.interrupt(cx);
                        self.session
                            .read(cx)
                            .state
                            .error
                            .clone()
                            .map_or(Ok(()), Err)
                    }
                }
                "/queue" => self.session.update(cx, CodexSession::resume_queue),
                "/compact" => self.session.update(cx, CodexSession::compact),
                "/clear" => self.session.update(cx, CodexSession::clear_conversation),
                _ => unreachable!(),
            }
        };
        if let Err(error) = result {
            self.session.update(cx, |session, cx| {
                session.state.error = Some(error);
                cx.notify();
            });
            return true;
        }
        if command == "/clear" {
            self.pending_clear = Some((
                self.session.read(cx).state.cleared_conversations,
                text,
                self.images.clone(),
            ));
            cx.notify();
            return true;
        }
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.session.update(cx, |session, cx| {
            session.state.error = None;
            session.send(Action::Draft(String::new()), cx);
        });
        cx.notify();
        true
    }
    fn refresh_files(&mut self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        let Some((_, '@', query)) = completion_trigger(&text, self.input.read(cx).cursor()) else {
            self.file_suggestions.clear();
            self.file_query.clear();
            self.file_lookup = None;
            return;
        };
        if self.file_lookup.as_deref() == Some(query) {
            return;
        }
        self.file_suggestions.clear();
        self.file_lookup = Some(query.to_owned());
        query.clone_into(&mut self.file_query);
        let query = query.to_owned();
        let cwd = self.session.read(cx).state.saved.cwd.clone();
        cx.spawn(async move |this, cx| {
            let lookup = query.clone();
            let files = cx
                .background_executor()
                .spawn(async move { file_choices(&cwd, &lookup) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.file_query == query {
                    this.file_suggestions = files;
                    cx.notify();
                }
            });
        })
        .detach();
    }
    fn attach_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.submitted.is_some() {
            return;
        }
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Attach files or folders".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let attachments = cx
                .background_executor()
                .spawn(async move {
                    let mut references = Vec::new();
                    let mut images = Vec::new();
                    for path in paths {
                        let format = path
                            .extension()
                            .and_then(|extension| extension.to_str())
                            .and_then(|extension| match extension.to_lowercase().as_str() {
                                "png" => Some(gpui::ImageFormat::Png),
                                "jpg" | "jpeg" => Some(gpui::ImageFormat::Jpeg),
                                "webp" => Some(gpui::ImageFormat::Webp),
                                "gif" => Some(gpui::ImageFormat::Gif),
                                "tiff" | "tif" => Some(gpui::ImageFormat::Tiff),
                                "bmp" => Some(gpui::ImageFormat::Bmp),
                                _ => None,
                            });
                        if let Some(format) = format {
                            if let Ok(bytes) = fs_err::read(&path) {
                                images.push(gpui::Image::from_bytes(format, bytes));
                                continue;
                            }
                        }
                        references.push(format!("@{}", path.display()));
                    }
                    (references.join("\n"), images)
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.submitted.is_some() {
                    return;
                }
                let (references, images) = attachments;
                if !images.is_empty() {
                    this.paste_images(images, window, cx);
                }
                if !references.is_empty() {
                    this.input.update(cx, |input, cx| {
                        input.replace(format!(" {references} "), window, cx);
                    });
                }
                let text = this.input.read(cx).value().to_string();
                this.session
                    .update(cx, |session, cx| session.send(Action::Draft(text), cx));
                this.input.focus_handle(cx).focus(window, cx);
                cx.notify();
            });
        })
        .detach();
    }
    fn toggle_expansion(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(key) {
            self.expanded.insert(key.to_owned());
        }
        cx.notify();
    }
    fn restore_image_tokens(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.validate_picked_tokens(cx);
        let text = self.input.read(cx).value();
        for number in 1..=self.images.len() {
            let value = format!("Image {number}");
            for (start, _) in text.match_indices(&value) {
                let range = start..start + value.len();
                if text[..range.start]
                    .chars()
                    .next_back()
                    .is_some_and(|c| !c.is_whitespace())
                    || text[range.end..]
                        .chars()
                        .next()
                        .is_some_and(|c| !c.is_whitespace())
                    || self.picked_tokens.iter().any(|token| {
                        token.value == value && token.decoration.get_ranges(cx).contains(&range)
                    })
                {
                    continue;
                }
                let decoration = self.input.update(cx, |input, cx| {
                    input.create_decorations_collection(
                        vec![TextDecoration::new(
                            range,
                            gpui::HighlightStyle {
                                background_color: Some(c::BG_HL()),
                                underline: Some(gpui::UnderlineStyle {
                                    thickness: rpx(COMPOSER_TOKEN_UNDERLINE)
                                        .to_pixels(window.rem_size()),
                                    color: Some(c::BORDER_STRONG()),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            },
                        )],
                        cx,
                    )
                });
                self.picked_tokens.push(ComposerToken {
                    value: value.clone(),
                    decoration,
                });
            }
        }
    }
    fn remove_image(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let mut edits = Vec::new();
        for token in &self.picked_tokens {
            if let Some(number) = token
                .value
                .strip_prefix("Image ")
                .and_then(|n| n.parse::<usize>().ok())
            {
                if number > index {
                    if let Some(range) = token.decoration.get_ranges(cx).first().cloned() {
                        edits.push((
                            range,
                            if number == index + 1 {
                                String::new()
                            } else {
                                format!("Image {}", number - 1)
                            },
                        ));
                    }
                }
            }
        }
        edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
        self.input.update(cx, |input, cx| {
            for (range, replacement) in edits {
                input.set_selected_range(range, cx);
                input.replace(replacement, window, cx);
            }
        });
        self.images.remove(index);
        self.restore_image_tokens(window, cx);
        let text = self.input.read(cx).value().to_string();
        if self.queue_editor.is_none() {
            self.session
                .update(cx, |session, cx| session.send(Action::Draft(text), cx));
        }
        self.persist_images(cx);
    }
    fn persist_images(&mut self, cx: &mut Context<Self>) {
        if self.queue_editor.is_some() {
            cx.notify();
            return;
        }
        self.session.update(cx, |session, cx| {
            session.send(Action::DraftImages(self.images.clone()), cx);
        });
        cx.notify();
    }
    fn paste_images(
        &mut self,
        images: Vec<gpui::Image>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.submitted.is_some() {
            return;
        }
        let editor = self.queue_editor.as_ref().map(|editor| editor.id);
        let cursor = self.input.read(cx).cursor();
        let original = self.input.read(cx).value();
        let previous = original[..cursor]
            .char_indices()
            .next_back()
            .map(|(offset, _)| offset);
        let anchor = previous.map(|start| {
            self.input.update(cx, |input, cx| {
                input.create_decorations_collection(
                    vec![TextDecoration::new(
                        start..cursor,
                        gpui::HighlightStyle::default(),
                    )],
                    cx,
                )
            })
        });
        self.pending_image_pastes += 1;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { prepare_images(images) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.pending_image_pastes -= 1;
                if this.queue_editor.as_ref().map(|editor| editor.id) != editor {
                    if let Some(anchor) = &anchor {
                        anchor.clear(cx);
                    }
                    cx.notify();
                    return;
                }
                match result {
                    Ok(images) => {
                        let size = this.images.iter().map(String::len).sum::<usize>()
                            + images.iter().map(|(url, _)| url.len()).sum::<usize>();
                        if this.images.len() + images.len() > 10 || size > 20 * 1024 * 1024 {
                            this.session.update(cx, |session, cx| {
                                session.state.error =
                                    Some("Attach up to 10 images, totaling at most 20 MiB.".into());
                                cx.notify();
                            });
                        } else {
                            let selection = this.input.read(cx).selected_range();
                            let insertion = anchor
                                .as_ref()
                                .and_then(|anchor| {
                                    anchor.get_ranges(cx).first().map(|range| range.end)
                                })
                                .unwrap_or(0)
                                .min(this.input.read(cx).value().len());
                            this.input.update(cx, |input, cx| {
                                input.set_selected_range(insertion..insertion, cx);
                            });
                            let mut inserted = 0;
                            for (url, image) in images {
                                let value = format!("Image {}", this.images.len() + 1);
                                let start = this.input.read(cx).cursor();
                                let prefix = if start > 0
                                    && !this.input.read(cx).value()[..start]
                                        .ends_with(char::is_whitespace)
                                {
                                    " "
                                } else {
                                    ""
                                };
                                this.input.update(cx, |input, cx| {
                                    input.insert(format!("{prefix}{value} "), window, cx);
                                });
                                let start = start + prefix.len();
                                let decoration = this.input.update(cx, |input, cx| {
                                    input.create_decorations_collection(
                                        vec![TextDecoration::new(
                                            start..start + value.len(),
                                            gpui::HighlightStyle {
                                                background_color: Some(c::BG_HL()),
                                                underline: Some(gpui::UnderlineStyle {
                                                    thickness: rpx(COMPOSER_TOKEN_UNDERLINE)
                                                        .to_pixels(window.rem_size()),
                                                    color: Some(c::BORDER_STRONG()),
                                                    ..Default::default()
                                                }),
                                                ..Default::default()
                                            },
                                        )],
                                        cx,
                                    )
                                });
                                inserted += prefix.len() + value.len() + 1;
                                this.picked_tokens.push(ComposerToken { value, decoration });
                                this.previews.insert(url.clone(), image);
                                this.images.push(url);
                            }
                            if selection != (cursor..cursor) {
                                let shift = |offset| {
                                    if offset >= insertion {
                                        offset + inserted
                                    } else {
                                        offset
                                    }
                                };
                                this.input.update(cx, |input, cx| {
                                    input.set_selected_range(
                                        shift(selection.start)..shift(selection.end),
                                        cx,
                                    );
                                });
                            }
                            let text = this.input.read(cx).value().to_string();
                            if this.queue_editor.is_none() {
                                this.session.update(cx, |session, cx| {
                                    session.send(Action::Draft(text), cx);
                                });
                            }
                            this.persist_images(cx);
                        }
                    }
                    Err(error) => this.session.update(cx, |session, cx| {
                        session.state.error = Some(error);
                        cx.notify();
                    }),
                }
                if let Some(anchor) = &anchor {
                    anchor.clear(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// During a turn only its latest activity is visible, including between calls.
/// Completion restores the work disclosure's full history.
fn tool_visible(state: &codex::Snapshot, index: usize) -> bool {
    if !state.status.busy() {
        return true;
    }
    let start = state
        .saved
        .work_periods
        .iter()
        .rev()
        .find(|period| period.end_item.is_none())
        .map_or_else(
            || {
                state
                    .saved
                    .items
                    .iter()
                    .rposition(|item| item["type"] == "userMessage")
                    .unwrap_or(0)
            },
            |period| period.start_item,
        );
    if index < start {
        return true;
    }
    state
        .saved
        .items
        .iter()
        .enumerate()
        .rev()
        .find(|(index, item)| *index >= start && is_tool_item(item))
        .is_some_and(|(current, _)| current == index)
}

/// Explicit message phases win; older providers keep their last response outside work.
fn work_commentary(state: &codex::Snapshot, index: usize) -> bool {
    let item = &state.saved.items[index];
    if item["type"] != "agentMessage" {
        return false;
    }
    match item["phase"].as_str() {
        Some("commentary") => true,
        Some("final_answer") => false,
        _ => state.saved.items[index + 1..]
            .iter()
            .take_while(|item| item["type"] != "userMessage")
            .any(|item| item["type"] == "agentMessage" || is_tool_item(item)),
    }
}

fn work_is_active_at(state: &codex::Snapshot, index: usize) -> bool {
    state.status.busy()
        && state
            .saved
            .work_periods
            .iter()
            .rev()
            .find(|period| period.start_item <= index)
            .map_or_else(
                || {
                    !state.saved.items[index + 1..]
                        .iter()
                        .any(|item| item["type"] == "userMessage")
                },
                |period| period.end_item.is_none(),
            )
}

fn work_key_at(state: &codex::Snapshot, index: usize) -> String {
    let start = state.saved.items[..(index + 1).min(state.saved.items.len())]
        .iter()
        .rposition(|item| item["type"] == "userMessage")
        .unwrap_or(0);
    format!("work-{start}")
}

/// Place work metadata after the user prompt, keeping the answer outside disclosure.
fn work_header_at(state: &codex::Snapshot, index: usize) -> Option<(String, String, bool)> {
    if state
        .saved
        .items
        .get(index)
        .is_some_and(|item| item["type"] == "userMessage")
    {
        return None;
    }
    let period =
        state.saved.work_periods.iter().rev().find(|period| {
            period.start_item <= index && period.end_item.is_none_or(|end| index < end)
        });
    let fallback_start = state.saved.items[..(index + 1).min(state.saved.items.len())]
        .iter()
        .rposition(|item| item["type"] == "userMessage")
        .unwrap_or(0);
    let start = period.map_or(fallback_start, |period| period.start_item);
    let latest_start = state
        .saved
        .items
        .iter()
        .rposition(|item| item["type"] == "userMessage")
        .unwrap_or(0);
    let active = state.status.busy()
        && period.map_or(start >= latest_start, |period| period.end_item.is_none());
    let end = period
        .and_then(|period| period.end_item)
        .unwrap_or(state.saved.items.len());
    let first = (start..end)
        .find(|index| state.saved.items[*index]["type"] != "userMessage")
        .or_else(|| active.then_some(state.saved.items.len()))?;
    if first != index {
        return None;
    }
    let duration = if active {
        state.work_started.map(|start| start.elapsed().as_secs())
    } else {
        period.and_then(|period| period.duration_ms.map(|ms| ms / 1000))
    };
    let verb = if active { "Working" } else { "Worked" };
    let label = duration.map_or_else(
        || verb.into(),
        |seconds| {
            let minutes = seconds / 60;
            if minutes == 0 {
                format!("{verb} for <1m")
            } else {
                format!("{verb} for {minutes}m")
            }
        },
    );
    Some((work_key_at(state, index), label, active))
}

fn tool_expansion_key(item: &serde_json::Value, index: usize) -> String {
    format!(
        "tool-{}",
        item["id"]
            .as_str()
            .map_or_else(|| index.to_string(), str::to_owned)
    )
}

fn tool_activity_label(label: String, index: usize, running: bool, cx: &App) -> gpui::AnyElement {
    shimmer_label(("codex-tool-shimmer", index).into(), label, running, cx)
}

fn tool_is_running(state: &codex::Snapshot, index: usize) -> bool {
    if !matches!(
        state.status,
        codex::Status::Starting | codex::Status::Working
    ) {
        return false;
    }
    let Some(item) = state.saved.items.get(index) else {
        return false;
    };
    item["status"] == "inProgress"
        // Claude reasoning has no item status; only its current trailing block is active.
        || (state.saved.provider == codex::Provider::Claude
            && item.get("status").is_none()
            && item["type"] == "reasoning"
            && index + 1 == state.saved.items.len())
}

/// A label whose glyphs carry a moving highlight while `running`; static with reduced motion.
fn shimmer_label(id: gpui::ElementId, label: String, running: bool, cx: &App) -> gpui::AnyElement {
    if !running || cx.reduce_motion() {
        return div().child(label).into_any_element();
    }
    div()
        .with_animation(
            id,
            gpui::Animation::new(TOOL_SHIMMER_PERIOD).repeat(),
            move |element, progress| {
                element.child(
                    gpui::StyledText::new(label.clone())
                        .with_highlights(shimmer_highlights(&label, progress)),
                )
            },
        )
        .into_any_element()
}

fn shimmer_highlights(
    text: &str,
    progress: f32,
) -> Vec<(std::ops::Range<usize>, gpui::HighlightStyle)> {
    let count = text.chars().count().max(1) as f32;
    let center = progress * (1.0 + TOOL_SHIMMER_WIDTH * 2.0) - TOOL_SHIMMER_WIDTH;
    let dim = c::FG_DIM();
    let bright = c::FG();
    text.char_indices()
        .enumerate()
        .map(|(index, (start, character))| {
            let strength =
                (1.0 - ((index as f32 / count - center).abs() / TOOL_SHIMMER_WIDTH)).max(0.0);
            let color = gpui::Hsla {
                h: dim.h + (bright.h - dim.h) * strength,
                s: dim.s + (bright.s - dim.s) * strength,
                l: dim.l + (bright.l - dim.l) * strength,
                a: dim.a + (bright.a - dim.a) * strength,
            };
            (
                start..start + character.len_utf8(),
                gpui::HighlightStyle {
                    color: Some(color),
                    ..Default::default()
                },
            )
        })
        .collect()
}

fn is_tool_item(item: &serde_json::Value) -> bool {
    !matches!(
        item["type"].as_str(),
        Some("userMessage" | "agentMessage" | "plan" | codex::COMPACTION_ITEM)
    )
}

/// Compaction is running: the provider said so, or its item has not completed yet.
fn compacting(state: &codex::Snapshot) -> bool {
    state.compacting
        || state.saved.items.last().is_some_and(|item| {
            item["type"] == codex::COMPACTION_ITEM && item["status"] == "inProgress"
        })
}

/// The live activity line's text, or `None` when the session is not working.
fn live_activity(state: &codex::Snapshot) -> Option<String> {
    let provider = state.saved.provider.label();
    if compacting(state) {
        return Some("Compacting context…".into());
    }
    if !state.status.busy() || state.saved.items.last().is_some_and(is_tool_item) {
        return None;
    }
    // A commentary update after a tool must not add a second working shimmer.
    // Actionable waiting copy and compaction retain their dedicated footer.
    if state.questions.is_empty()
        && state.elicitations.is_empty()
        && state.approvals.is_empty()
        && state.saved.items.iter().enumerate().any(|(index, item)| {
            is_tool_item(item) && work_is_active_at(state, index) && tool_visible(state, index)
        })
    {
        return None;
    }
    Some(
        if !state.questions.is_empty() || !state.elicitations.is_empty() {
            "Waiting for your answers…".into()
        } else if !state.approvals.is_empty() {
            "Waiting for your permission…".into()
        } else {
            state.saved.items.last().map_or_else(
                || {
                    if state.saved.thread_id.is_none() {
                        format!("Connecting to {provider}…")
                    } else {
                        format!("{provider} is working…")
                    }
                },
                |item| match item["type"].as_str().unwrap_or("") {
                    "commandExecution" => {
                        format!("Running {}", item["command"].as_str().unwrap_or("command"))
                    }
                    "fileChange" => "Updating files…".into(),
                    "mcpToolCall" => "Using a connected tool…".into(),
                    "webSearch" => "Searching the web…".into(),
                    "reasoning" => "Thinking…".into(),
                    _ => format!("{provider} is working…"),
                },
            )
        },
    )
}

/// A centred transcript divider between hairline rules, e.g. "Context compacted · auto".
fn compaction_divider(index: usize, label: String) -> impl IntoElement {
    let rule = || {
        div()
            .flex_1()
            .min_w_0()
            .h(gpui::px(1.))
            .bg(c::BORDER_SOFT())
    };
    div()
        .id(("codex-compaction", index))
        .debug_selector(move || format!("codex-compaction-{index}"))
        .w_full()
        .max_w(rpx(CHAT_MAX_W))
        .flex()
        .items_center()
        .gap(rpx(SPACE_2XL))
        .text_size(rpx(TEXT_SMALL))
        .text_color(c::FG_DIM())
        .aria_label(label.clone())
        .child(rule())
        .child(div().flex_shrink_0().child(label))
        .child(rule())
}

/// The meter's whole percent and colour: muted at rest, warning at the threshold.
fn context_tone(usage: codex::ContextUsage) -> (u32, gpui::Hsla) {
    let percent = usage.percent();
    let color = if percent >= CONTEXT_WARN_PERCENT {
        c::AMBER()
    } else {
        c::FG_MUTE()
    };
    (percent, color)
}

/// The meter's hover detail, e.g. "76k / 200k tokens".
fn context_detail(usage: codex::ContextUsage) -> String {
    format!(
        "{} / {} tokens",
        codex::compact_tokens(usage.used),
        codex::compact_tokens(usage.window)
    )
}

/// Context-window occupancy: a vector progress ring and its percentage.
fn context_meter(usage: codex::ContextUsage) -> impl IntoElement {
    let (percent, color) = context_tone(usage);
    let detail = context_detail(usage);
    div()
        .id("codex-context-meter")
        .debug_selector(|| "codex-context-meter".into())
        .role(gpui::Role::Meter)
        .aria_label(format!("Context {percent}% used, {detail}"))
        .flex_shrink_0()
        .h(rpx(CONTROL_H))
        .flex()
        .items_center()
        .gap(rpx(SPACE_SM))
        .text_size(rpx(TEXT_SMALL))
        .text_color(color)
        .tooltip(move |window, cx| tooltip(detail.clone(), window).build(window, cx))
        .child(context_ring(percent as f32 / 100.0, color))
        .child(format!("{percent}%"))
}

/// A track circle with a clockwise arc from twelve o'clock. Drawn as vector paths because
/// the bundled fonts carry no geometric-shape glyphs.
fn context_ring(fraction: f32, color: gpui::Hsla) -> impl IntoElement {
    let track = c::BORDER();
    gpui::canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let stroke = rpx(CONTEXT_RING_STROKE).to_pixels(window.rem_size());
            let center = bounds.center();
            let radius = bounds.size.width.min(bounds.size.height) / 2.0 - stroke / 2.0;
            let arc = |sweep: f32| {
                let steps = (CONTEXT_RING_SEGMENTS * sweep).ceil().max(1.0) as usize;
                let mut path = gpui::PathBuilder::stroke(stroke);
                for step in 0..=steps {
                    let angle = std::f32::consts::TAU * sweep * step as f32 / steps as f32
                        - std::f32::consts::FRAC_PI_2;
                    let point = gpui::point(
                        center.x + radius * angle.cos(),
                        center.y + radius * angle.sin(),
                    );
                    if step == 0 {
                        path.move_to(point);
                    } else {
                        path.line_to(point);
                    }
                }
                if sweep >= 1.0 {
                    path.close();
                }
                path.build().ok()
            };
            if let Some(path) = arc(1.0) {
                window.paint_path(path, track);
            }
            if fraction > 0.0 {
                if let Some(path) = arc(fraction.min(1.0)) {
                    window.paint_path(path, color);
                }
            }
        },
    )
    .size(rpx(ICON_MD))
    .flex_shrink_0()
}

/// Quiet activity rows describe the invocation; output stays behind disclosure.
fn tool_activity_summary(item: &serde_json::Value, running: bool) -> String {
    if item["type"] == "reasoning" {
        return if running { "Thinking…" } else { "Thought" }.into();
    }
    if item["type"] == "commandExecution" {
        let command = item["command"].as_str().unwrap_or("command");
        return format!(
            "{} {}",
            if running { "Running" } else { "Ran" },
            command.split_whitespace().collect::<Vec<_>>().join(" ")
        );
    }
    tool_summary(item)
}

fn tool_summary(item: &serde_json::Value) -> String {
    let summary = match item["type"].as_str().unwrap_or_default() {
        "commandExecution" => format!(
            "Command · {}",
            item["command"].as_str().unwrap_or("Starting command…")
        ),
        "fileChange" => {
            let paths = item["changes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|change| change["path"].as_str())
                .collect::<Vec<_>>();
            if paths.is_empty() {
                "File changes".into()
            } else {
                format!("File changes · {}", paths.join(", "))
            }
        }
        "mcpToolCall" => format!(
            "Tool · {} / {}",
            item["server"].as_str().unwrap_or("Connected tool"),
            item["tool"].as_str().unwrap_or("Starting…")
        ),
        "webSearch" => format!(
            "Web search · {}",
            item["action"]["query"]
                .as_str()
                .or(item["query"].as_str())
                .unwrap_or("Searching…")
        ),
        "reasoning" => "Thinking…".into(),
        _ => item["tool"]
            .as_str()
            .or(item["type"].as_str())
            .unwrap_or("Tool call")
            .to_owned(),
    };
    // Keep multiline commands and queries in the disclosure's single summary row.
    summary.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn tool_detail(item: &serde_json::Value) -> String {
    let text = codex::item_text(item);
    match item["type"].as_str().unwrap_or_default() {
        "mcpToolCall" => {
            let arguments = item.get("arguments").or_else(|| item.get("input"));
            match arguments {
                Some(arguments) => format!(
                    "Arguments:\n{}\n\n{text}",
                    serde_json::to_string_pretty(arguments).unwrap_or_default()
                ),
                None => text,
            }
        }
        "commandExecution" | "fileChange" | "reasoning" | "webSearch" => text,
        _ => serde_json::to_string_pretty(item).unwrap_or(text),
    }
}

pub(super) fn markdown_style(window: &Window) -> TextViewStyle {
    let mut style = TextViewStyle::default();
    style.is_dark = c::is_dark();
    style.heading_base_font_size = rpx(TEXT_BODY).to_pixels(window.rem_size());
    style.highlight_theme = if c::is_dark() {
        HighlightTheme::default_dark().clone()
    } else {
        HighlightTheme::default_light().clone()
    };
    style.paragraph_gap = rpx(SPACE_2XL);
    style.code_block = div()
        .bg(c::FIELD_FILL())
        .text_color(c::FG())
        .p(rpx(SPACE_2XL))
        .rounded(rpx(RADIUS_GROUP))
        .text_size(rpx(TEXT_CODE))
        .style()
        .clone();
    style
}
fn prepare_images(images: Vec<gpui::Image>) -> Result<Vec<(String, Arc<gpui::Image>)>, String> {
    use std::io::Cursor;
    if images.len() > 10
        || images
            .iter()
            .map(|image| image.bytes().len())
            .sum::<usize>()
            > 20 * 1024 * 1024
    {
        return Err("Attach up to 10 images, totaling at most 20 MiB.".into());
    }
    images
        .into_iter()
        .map(|image| {
            let format = match image.format {
                gpui::ImageFormat::Png => image::ImageFormat::Png,
                gpui::ImageFormat::Jpeg => image::ImageFormat::Jpeg,
                gpui::ImageFormat::Webp => image::ImageFormat::WebP,
                gpui::ImageFormat::Gif => image::ImageFormat::Gif,
                gpui::ImageFormat::Tiff => image::ImageFormat::Tiff,
                gpui::ImageFormat::Bmp => image::ImageFormat::Bmp,
                _ => return Err("Paste a PNG, JPEG, WebP, GIF, TIFF, or BMP image.".into()),
            };
            let mut reader = image::ImageReader::with_format(Cursor::new(image.bytes()), format);
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(8192);
            limits.max_image_height = Some(8192);
            limits.max_alloc = Some(128 * 1024 * 1024);
            reader.limits(limits);
            let decoded = reader
                .decode()
                .map_err(|error| format!("Couldn't read pasted image: {error}"))?;
            let image = if matches!(
                image.format,
                gpui::ImageFormat::Tiff | gpui::ImageFormat::Bmp
            ) {
                let mut bytes = Cursor::new(Vec::new());
                decoded
                    .write_to(&mut bytes, image::ImageFormat::Png)
                    .map_err(|error| format!("Couldn't prepare pasted image: {error}"))?;
                gpui::Image::from_bytes(gpui::ImageFormat::Png, bytes.into_inner())
            } else {
                image
            };
            let url = format!(
                "data:{};base64,{}",
                image.format.mime_type(),
                STANDARD.encode(image.bytes())
            );
            Ok((url, Arc::new(image)))
        })
        .collect()
}

fn image_from_url(url: &str) -> Option<Arc<gpui::Image>> {
    let (mime, encoded) = url.strip_prefix("data:")?.split_once(";base64,")?;
    let format = gpui::ImageFormat::from_mime_type(mime)?;
    Some(Arc::new(gpui::Image::from_bytes(
        format,
        STANDARD.decode(encoded).ok()?,
    )))
}

fn effective_model(state: &codex::Snapshot) -> Option<&serde_json::Value> {
    let selected = state.model.as_deref().or(state.resolved_model.as_deref())?;
    state
        .models
        .iter()
        .find(|model| model["model"].as_str() == Some(selected))
        .or_else(|| {
            (state.saved.provider == codex::Provider::Claude)
                .then(|| {
                    state
                        .models
                        .iter()
                        .find(|model| model["resolvedModel"].as_str() == Some(selected))
                })
                .flatten()
        })
}

/// Host commands first, then (for Claude) the CLI's own commands Grove does not host.
fn slash_choices(state: &codex::Snapshot, query: &str) -> Vec<(String, String)> {
    let mut choices: Vec<(String, String)> = [
        ("/model", "Choose model"),
        ("/effort", "Choose reasoning effort"),
        ("/access", "Choose access level"),
        ("/stop", "Interrupt the current turn"),
        ("/compact", "Compact conversation context"),
        ("/queue", "Send the next queued prompt"),
        ("/clear", "Start a new conversation"),
    ]
    .into_iter()
    .filter(|(command, _)| command[1..].starts_with(query))
    .map(|(command, label)| (command.into(), format!("{command} · {label}")))
    .collect();
    if state.saved.provider == codex::Provider::Claude {
        choices.extend(state.skills.iter().filter_map(|skill| {
            let name = skill["name"].as_str()?;
            (name.starts_with(query)
                && !host_command(name)
                && skill["enabled"].as_bool() != Some(false))
            .then(|| {
                (
                    format!("/{name}"),
                    format!(
                        "/{name} · {}",
                        skill["description"].as_str().unwrap_or("Claude command")
                    ),
                )
            })
        }));
    }
    choices
}

fn host_command(name: &str) -> bool {
    matches!(
        name,
        "model" | "effort" | "access" | "stop" | "queue" | "clear" | "compact"
    )
}

fn composer_placeholder(provider: codex::Provider) -> &'static str {
    if provider == codex::Provider::Claude {
        "Ask anything, @tag files/folders, or / for skills and commands"
    } else {
        "Ask anything, @tag files/folders, $use skills, or / for commands"
    }
}

/// Retain the first line while explicitly indicating omitted draft lines.
fn collapsed_draft_preview(text: &str) -> String {
    match text.split_once('\n') {
        Some((first, _)) => format!("{first}…"),
        None => text.to_owned(),
    }
}

/// Complete the token around a collapsed caret without replacing later draft text.
fn completion_trigger(text: &str, cursor: usize) -> Option<(std::ops::Range<usize>, char, &str)> {
    let prefix = text.get(..cursor)?;
    let start = prefix.rfind(char::is_whitespace).map_or(0, |at| at + 1);
    let token = &prefix[start..];
    let trigger = token.chars().next()?;
    if !(matches!(trigger, '@' | '$') || trigger == '/' && start == 0) {
        return None;
    }
    let end = text[cursor..]
        .find(char::is_whitespace)
        .map_or(text.len(), |at| cursor + at);
    Some((start..end, trigger, &token[trigger.len_utf8()..]))
}

fn trailing_trigger(text: &str) -> Option<(char, &str)> {
    let token = text.rsplit(char::is_whitespace).next()?;
    let trigger = token.chars().next()?;
    if matches!(trigger, '@' | '$') || (trigger == '/' && token == text) {
        Some((trigger, &token[trigger.len_utf8()..]))
    } else {
        None
    }
}
fn file_choices(cwd: &str, query: &str) -> Vec<String> {
    let path = std::path::Path::new(query);
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new(""));
    let (directory, prefix) = if query.ends_with('/') {
        (path, "")
    } else {
        (
            parent,
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(""),
        )
    };
    let Ok(entries) = fs_err::read_dir(std::path::Path::new(cwd).join(directory)) else {
        return Vec::new();
    };
    let mut choices = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.to_lowercase().starts_with(&prefix.to_lowercase())
                || (name.starts_with('.') && !prefix.starts_with('.'))
            {
                return None;
            }
            let mut display = directory.join(name).to_string_lossy().to_string();
            if entry.file_type().ok()?.is_dir() {
                display.push('/');
            }
            Some(display)
        })
        .take(100)
        .collect::<Vec<_>>();
    choices.sort();
    choices.truncate(8);
    choices
}

impl CodexView {
    fn elicitation_key(request: &ElicitationRequest) -> String {
        format!("{request:?}")
    }
    fn sync_elicitations(
        &mut self,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keys: HashSet<_> = state
            .elicitations
            .iter()
            .map(Self::elicitation_key)
            .collect();
        let restore_focus = self.elicitation_drafts.iter().any(|(key, draft)| {
            !keys.contains(key)
                && draft.fields.iter().any(|field| {
                    field.input.focus_handle(cx).is_focused(window)
                        || field
                            .choice_focus
                            .values()
                            .any(|focus| focus.is_focused(window))
                })
        });
        self.elicitation_drafts.retain(|key, _| keys.contains(key));
        if restore_focus {
            self.input.focus_handle(cx).focus(window, cx);
        }
        for request in &state.elicitations {
            let key = Self::elicitation_key(request);
            if let Some(draft) = self.elicitation_drafts.get_mut(&key) {
                if state.error.is_some() {
                    draft.submitting = false;
                }
                continue;
            }
            let fields = match &request.mode {
                ElicitationMode::Form(form) => form
                    .fields
                    .iter()
                    .map(|field| {
                        let value = field.default.clone().unwrap_or(serde_json::Value::Null);
                        let initial = value.as_str().map_or_else(
                            || {
                                if value.is_number() {
                                    value.to_string()
                                } else {
                                    String::new()
                                }
                            },
                            str::to_owned,
                        );
                        let input = cx.new(|cx| InputState::new(window, cx).default_value(initial));
                        let subscription = cx.observe(&input, |_, _, cx| cx.notify());
                        let selected = if let Some(values) = value.as_array() {
                            values
                                .iter()
                                .filter_map(|v| v.as_str().map(str::to_owned))
                                .collect()
                        } else {
                            value.as_str().map(str::to_owned).into_iter().collect()
                        };
                        let choice_focus = match &field.kind {
                            ElicitationKind::Select { options }
                            | ElicitationKind::MultiSelect { options, .. } => options
                                .iter()
                                .map(|option| (option.value.clone(), cx.focus_handle()))
                                .collect(),
                            _ => HashMap::new(),
                        };
                        ElicitationFieldDraft {
                            input,
                            selected,
                            choice_focus,
                            included: field.required || field.default.is_some(),
                            boolean: value.as_bool().unwrap_or(false),
                            _subscription: subscription,
                        }
                    })
                    .collect(),
                ElicitationMode::Url { .. } => Vec::new(),
            };
            self.elicitation_drafts.insert(
                key,
                ElicitationDraft {
                    fields,
                    submitting: false,
                },
            );
        }
    }
    fn elicitation_content(
        &self,
        key: &str,
        request: &ElicitationRequest,
        cx: &App,
    ) -> Result<serde_json::Value, String> {
        let ElicitationMode::Form(form) = &request.mode else {
            return Ok(serde_json::Value::Null);
        };
        let draft = self.elicitation_drafts.get(key).ok_or("Request expired")?;
        let mut content = serde_json::Map::new();
        for (field, draft) in form.fields.iter().zip(&draft.fields) {
            if !draft.included {
                continue;
            }
            let text = draft.input.read(cx).value().to_string();
            let value = match &field.kind {
                ElicitationKind::String { .. } => serde_json::Value::String(text),
                ElicitationKind::Number { integer, .. } => {
                    let number = serde_json::from_str::<serde_json::Value>(text.trim())
                        .ok()
                        .filter(|v| {
                            v.is_number()
                                && (!integer || v.as_i64().is_some() || v.as_u64().is_some())
                        });
                    number.ok_or_else(|| {
                        format!(
                            "{}: enter {}",
                            field.title,
                            if *integer {
                                "a whole number"
                            } else {
                                "a number"
                            }
                        )
                    })?
                }
                ElicitationKind::Boolean => serde_json::Value::Bool(draft.boolean),
                ElicitationKind::Select { options } => options
                    .iter()
                    .find(|option| draft.selected.contains(&option.value))
                    .map(|option| serde_json::Value::String(option.value.clone()))
                    .ok_or_else(|| format!("{}: choose an option", field.title))?,
                ElicitationKind::MultiSelect { options, .. } => serde_json::Value::Array(
                    options
                        .iter()
                        .filter(|option| draft.selected.contains(&option.value))
                        .map(|option| serde_json::Value::String(option.value.clone()))
                        .collect(),
                ),
            };
            content.insert(field.name.clone(), value);
        }
        form.validate(&content).map_err(|errors| {
            errors
                .into_iter()
                .map(|error| format!("{}: {}", error.field, error.message))
                .collect::<Vec<_>>()
                .join("; ")
        })
    }
    fn change_elicitation(
        &mut self,
        key: &str,
        index: usize,
        value: Option<String>,
        multiple: bool,
        include: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self.elicitation_drafts.get_mut(key) else {
            return;
        };
        if group.submitting {
            return;
        }
        let field = &mut group.fields[index];
        if let Some(value) = value {
            if multiple {
                if !field.selected.remove(&value) {
                    field.selected.insert(value);
                }
            } else {
                field.selected.clear();
                field.selected.insert(value);
            }
        } else if include {
            field.included = !field.included;
        } else {
            field.boolean = !field.boolean;
        }
        cx.notify();
    }
    fn respond_elicitation(
        &mut self,
        key: &str,
        id: serde_json::Value,
        action: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self
            .session
            .read(cx)
            .state
            .elicitations
            .iter()
            .find(|request| request.id == id && Self::elicitation_key(request) == key)
        else {
            return;
        };
        let content = if action == "accept" && matches!(request.mode, ElicitationMode::Form(_)) {
            let Ok(content) = self.elicitation_content(key, request, cx) else {
                return;
            };
            Some(content)
        } else {
            None
        };
        let Some(draft) = self.elicitation_drafts.get_mut(key) else {
            return;
        };
        if draft.submitting {
            return;
        }
        draft.submitting = true;
        self.session.update(cx, |session, cx| {
            session.send(
                Action::RespondElicitation {
                    id,
                    action: action.to_owned(),
                    content,
                },
                cx,
            );
        });
        cx.notify();
    }
    fn elicitation_panel(
        &self,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut panel = div()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_3XL));
        for (request_index, request) in state.elicitations.iter().enumerate() {
            let key = Self::elicitation_key(request);
            let draft = &self.elicitation_drafts[&key];
            let mut group = div()
                .id(("agent-elicitation", request_index))
                .role(gpui::Role::Group)
                .aria_label(format!("Input requested by {}", request.server_name))
                .w_full()
                .min_w_0()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .child(
                    div()
                        .text_color(c::AMBER())
                        .child(format!("{} needs your input", request.server_name)),
                )
                .child(div().child(request.message.clone()));
            let valid = self.elicitation_content(&key, request, cx);
            match &request.mode {
                ElicitationMode::Url { url, .. } => {
                    let open_url = url.clone();
                    group = group
                        .child(div().text_size(rpx(TEXT_SMALL)).child(url.clone()))
                        .child(
                            form_action(
                                gpui::SharedString::from(format!(
                                    "elicitation-open-{request_index}"
                                )),
                                "Open link",
                                false,
                                window,
                                cx,
                            )
                            .disabled(draft.submitting)
                            .on_click(move |_, _, cx| {
                                if grove_core::elicitation::validate_elicitation_url(&open_url)
                                    .is_ok()
                                {
                                    cx.open_url(&open_url);
                                }
                            }),
                        )
                        .child(
                            div()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child(
                                "Finish the requested step in your browser, then confirm below.",
                            ),
                        );
                }
                ElicitationMode::Form(form) => {
                    for (index, field) in form.fields.iter().enumerate() {
                        let field_draft = &draft.fields[index];
                        let mut row = div()
                            .id(gpui::SharedString::from(format!(
                                "elicitation-field-{request_index}-{index}"
                            )))
                            .role(if matches!(field.kind, ElicitationKind::Select { .. }) {
                                gpui::Role::RadioGroup
                            } else {
                                gpui::Role::Group
                            })
                            .aria_label(field.title.clone())
                            .w_full()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_SM))
                            .child(div().child(format!(
                                "{}{}",
                                field.title,
                                if field.required {
                                    " (required)"
                                } else {
                                    " (optional)"
                                }
                            )));
                        if let Some(description) = &field.description {
                            row = row.child(
                                div()
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_DIM())
                                    .child(description.clone()),
                            );
                        }
                        if !field.required {
                            row = row.child(self.elicitation_choice(
                                &key,
                                index,
                                request_index,
                                ElicitationChoice::Include,
                                format!("Provide {}", field.title),
                                field_draft.included,
                                draft.submitting,
                                cx,
                            ));
                        }
                        if field_draft.included {
                            match &field.kind {
                                ElicitationKind::Boolean => {
                                    row = row.child(self.elicitation_choice(
                                        &key,
                                        index,
                                        request_index,
                                        ElicitationChoice::Boolean,
                                        field.title.clone(),
                                        field_draft.boolean,
                                        draft.submitting,
                                        cx,
                                    ));
                                }
                                ElicitationKind::Select { options }
                                | ElicitationKind::MultiSelect { options, .. } => {
                                    let multiple =
                                        matches!(field.kind, ElicitationKind::MultiSelect { .. });
                                    for option in options {
                                        row = row.child(self.elicitation_choice(
                                            &key,
                                            index,
                                            request_index,
                                            ElicitationChoice::Option(
                                                option.value.clone(),
                                                multiple,
                                            ),
                                            option.title.clone(),
                                            field_draft.selected.contains(&option.value),
                                            draft.submitting,
                                            cx,
                                        ));
                                    }
                                }
                                ElicitationKind::String { .. } | ElicitationKind::Number { .. } => {
                                    row = row.child(
                                        project_field_well(
                                            field_draft.input.focus_handle(cx).is_focused(window),
                                            false,
                                        )
                                        .child(
                                            Input::new(&field_draft.input)
                                                .aria_label(field.title.clone())
                                                .disabled(draft.submitting)
                                                .appearance(false)
                                                .bordered(false)
                                                .focus_bordered(false)
                                                .p_0(),
                                        ),
                                    );
                                }
                            }
                        }
                        group = group.child(row);
                    }
                    if let Err(error) = &valid {
                        group = group.child(
                            div()
                                .id(gpui::SharedString::from(format!(
                                    "elicitation-validation-{request_index}"
                                )))
                                .role(gpui::Role::Alert)
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child(error.clone()),
                        );
                    }
                }
            }
            let mut buttons = div().flex().flex_wrap().gap(rpx(SPACE_LG));
            for (action, label) in [
                ("cancel", "Cancel"),
                ("decline", "Decline"),
                (
                    "accept",
                    if matches!(request.mode, ElicitationMode::Url { .. }) {
                        "I've completed this"
                    } else {
                        "Submit"
                    },
                ),
            ] {
                let click_key = key.clone();
                let id = request.id.clone();
                buttons = buttons.child(
                    form_action(
                        gpui::SharedString::from(format!("elicitation-{request_index}-{action}")),
                        label,
                        action == "accept",
                        window,
                        cx,
                    )
                    .disabled(draft.submitting || (action == "accept" && valid.is_err()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.respond_elicitation(&click_key, id.clone(), action, cx);
                    })),
                );
            }
            panel = panel.child(group.child(buttons));
        }
        panel
    }
    fn move_elicitation_choice(
        &mut self,
        key: &str,
        index: usize,
        current: &str,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self
            .session
            .read(cx)
            .state
            .elicitations
            .iter()
            .find(|request| Self::elicitation_key(request) == key)
        else {
            return;
        };
        let ElicitationMode::Form(form) = &request.mode else {
            return;
        };
        let ElicitationKind::Select { options } = &form.fields[index].kind else {
            return;
        };
        let Some(position) = options.iter().position(|option| option.value == current) else {
            return;
        };
        let next = if forward {
            (position + 1) % options.len()
        } else {
            (position + options.len() - 1) % options.len()
        };
        let value = options[next].value.clone();
        if let Some(draft) = self.elicitation_drafts.get(key) {
            if draft.submitting {
                return;
            }
            if let Some(focus) = draft.fields[index].choice_focus.get(&value) {
                focus.focus(window, cx);
            }
        }
        self.change_elicitation(key, index, Some(value), false, false, cx);
    }
    #[allow(clippy::too_many_arguments)]
    fn elicitation_choice(
        &self,
        key: &str,
        index: usize,
        request_index: usize,
        choice: ElicitationChoice,
        label: String,
        selected: bool,
        submitting: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let click_key = key.to_owned();
        let keyboard_key = key.to_owned();
        let (value, multiple, include) = match choice {
            ElicitationChoice::Include => (None, true, true),
            ElicitationChoice::Boolean => (None, true, false),
            ElicitationChoice::Option(value, multiple) => (Some(value), multiple, false),
        };
        let identity = format!(
            "elicitation-choice-{request_index}-{index}-{include}-{}",
            value.as_deref().unwrap_or("toggle")
        );
        let keyboard_value = value.clone();
        let focus = value
            .as_ref()
            .and_then(|value| {
                self.elicitation_drafts[key].fields[index]
                    .choice_focus
                    .get(value)
            })
            .cloned();
        question_choice(
            gpui::SharedString::from(identity),
            label,
            String::new(),
            multiple,
            selected,
        )
        .when_some(focus, |row, focus| row.track_focus(&focus))
        .when(submitting, |row| row.opacity(OPACITY_DISABLED))
        .on_click(cx.listener(move |this, _, _, cx| {
            this.change_elicitation(&click_key, index, value.clone(), multiple, include, cx);
        }))
        .on_key_down(
            cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                if !event.is_held
                    && !multiple
                    && matches!(
                        event.keystroke.key.as_str(),
                        "up" | "left" | "down" | "right"
                    )
                {
                    if let Some(current) = &keyboard_value {
                        this.move_elicitation_choice(
                            &keyboard_key,
                            index,
                            current,
                            matches!(event.keystroke.key.as_str(), "down" | "right"),
                            window,
                            cx,
                        );
                        cx.stop_propagation();
                    }
                }
                if !event.is_held && matches!(event.keystroke.key.as_str(), "space" | "enter") {
                    this.change_elicitation(
                        &keyboard_key,
                        index,
                        keyboard_value.clone(),
                        multiple,
                        include,
                        cx,
                    );
                    cx.stop_propagation();
                }
            }),
        )
    }
    fn sync_approvals(
        &mut self,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keys: HashSet<_> = state.approvals.iter().map(Self::approval_key).collect();
        let restore_focus = self.approval_drafts.iter().any(|(key, draft)| {
            !keys.contains(key)
                && (draft.input.focus_handle(cx).is_focused(window)
                    || draft.feedback.focus_handle(cx).is_focused(window))
        });
        self.approval_drafts.retain(|key, _| keys.contains(key));
        if restore_focus {
            self.input.focus_handle(cx).focus(window, cx);
        }
        for approval in &state.approvals {
            let key = Self::approval_key(approval);
            if let Some(draft) = self.approval_drafts.get_mut(&key) {
                if state.error.is_some() {
                    draft.submitting = false;
                }
                continue;
            }
            let input = cx.new(|cx| {
                InputState::new(window, cx).auto_grow(3, 10).default_value(
                    serde_json::to_string_pretty(&approval["claudeInput"]).unwrap_or_default(),
                )
            });
            let feedback = cx.new(|cx| {
                InputState::new(window, cx).placeholder("e.g. Please use the project folder only")
            });
            let subscriptions = vec![
                cx.observe(&input, |_, _, cx| cx.notify()),
                cx.observe(&feedback, |_, _, cx| cx.notify()),
            ];
            self.approval_drafts.insert(
                key,
                ApprovalDraft {
                    input,
                    feedback,
                    editing: false,
                    submitting: false,
                    _subscriptions: subscriptions,
                },
            );
        }
    }
    fn approval_key(approval: &serde_json::Value) -> String {
        approval.to_string()
    }
    fn respond_approval(
        &mut self,
        key: &str,
        id: serde_json::Value,
        choice: String,
        cx: &mut Context<Self>,
    ) {
        let state = &self.session.read(cx).state;
        let Some(request) = state
            .approvals
            .iter()
            .find(|request| Self::approval_key(request) == key && request["id"] == id)
        else {
            return;
        };
        if !grove_core::controls::approval_choices(request)
            .iter()
            .any(|option| option.id == choice)
        {
            return;
        }
        let Some(draft) = self.approval_drafts.get_mut(key) else {
            return;
        };
        if draft.submitting {
            return;
        }
        let denying = matches!(choice.as_str(), "deny" | "cancel");
        let input = if draft.editing && !denying {
            match serde_json::from_str::<serde_json::Value>(&draft.input.read(cx).value()) {
                Ok(value) if value.is_object() => Some(value),
                _ => return,
            }
        } else {
            None
        };
        let feedback = denying
            .then(|| draft.feedback.read(cx).value().trim().to_owned())
            .filter(|text| !text.is_empty());
        draft.submitting = true;
        self.session.update(cx, |session, cx| {
            session.send(
                Action::RespondApproval {
                    id,
                    choice,
                    input,
                    feedback,
                },
                cx,
            );
        });
        cx.notify();
    }
    fn approval_panel(
        &self,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut panel = div()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_3XL));
        for (index, approval) in state.approvals.iter().enumerate() {
            let key = Self::approval_key(approval);
            let draft = &self.approval_drafts[&key];
            let mut group = div()
                .id(("codex-approval", index))
                .role(gpui::Role::Group)
                .aria_label(grove_core::controls::approval_title(approval))
                .w_full()
                .min_w_0()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .child(div().text_color(c::AMBER()).child(format!(
                    "{} needs your permission · {}",
                    state.saved.provider.label(),
                    grove_core::controls::approval_title(approval)
                )))
                .child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .child(grove_core::controls::approval_details(approval)),
                );
            let editable = approval
                .get("claudeInput")
                .is_some_and(serde_json::Value::is_object);
            let valid_json = !draft.editing
                || serde_json::from_str::<serde_json::Value>(&draft.input.read(cx).value())
                    .is_ok_and(|value| value.is_object());
            if editable {
                let click_key = key.clone();
                let keyboard_key = key.clone();
                group = group.child(
                    question_choice(
                        gpui::SharedString::from(format!("approval-edit-{index}")),
                        "Edit tool input".into(),
                        "Applies only when you approve this request".into(),
                        true,
                        draft.editing,
                    )
                    .when(draft.submitting, |row| row.opacity(OPACITY_DISABLED))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_approval_edit(&click_key, cx);
                    }))
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, _, cx| {
                            if !event.is_held
                                && matches!(event.keystroke.key.as_str(), "space" | "enter")
                            {
                                this.toggle_approval_edit(&keyboard_key, cx);
                                cx.stop_propagation();
                            }
                        },
                    )),
                );
                if draft.editing {
                    group = group.child(
                        project_field_well(
                            draft.input.focus_handle(cx).is_focused(window),
                            !valid_json,
                        )
                        .child(
                            Input::new(&draft.input)
                                .aria_label("Tool input JSON")
                                .disabled(draft.submitting)
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false)
                                .p_0(),
                        ),
                    );
                    if !valid_json {
                        group = group.child(
                            div()
                                .id(gpui::SharedString::from(format!(
                                    "approval-validation-{index}"
                                )))
                                .role(gpui::Role::Alert)
                                .text_color(c::RED())
                                .child("Enter a valid JSON object before approving."),
                        );
                    }
                }
                group = group
                    .child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child("Feedback if denied (optional)"),
                    )
                    .child(
                        project_field_well(
                            draft.feedback.focus_handle(cx).is_focused(window),
                            false,
                        )
                        .child(
                            Input::new(&draft.feedback)
                                .aria_label("Feedback if denied")
                                .disabled(draft.submitting)
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false)
                                .p_0(),
                        ),
                    );
            }
            let mut buttons = div().flex().flex_wrap().gap(rpx(SPACE_LG));
            for (choice_index, choice) in grove_core::controls::approval_choices(approval)
                .into_iter()
                .enumerate()
            {
                let click_key = key.clone();
                let id = approval["id"].clone();
                let denying = matches!(choice.id.as_str(), "deny" | "cancel");
                buttons = buttons.child(
                    form_action(
                        gpui::SharedString::from(format!("approval-{index}-{choice_index}")),
                        choice.label,
                        false,
                        window,
                        cx,
                    )
                    .disabled(draft.submitting || (!denying && !valid_json))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.respond_approval(&click_key, id.clone(), choice.id.clone(), cx);
                    })),
                );
            }
            panel = panel.child(group.child(buttons));
        }
        panel
    }
    fn toggle_approval_edit(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(draft) = self.approval_drafts.get_mut(key) {
            if !draft.submitting {
                draft.editing = !draft.editing;
                cx.notify();
            }
        }
    }
    fn sync_questions(
        &mut self,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.question_generation != state.cleared_conversations
            || (self.stream_thread.is_some() && self.stream_thread != state.saved.thread_id)
        {
            self.question_receipts.clear();
            self.question_drafts.clear();
            self.question_generation = state.cleared_conversations;
        }
        let keys: HashSet<_> = state
            .questions
            .iter()
            .map(|r| question_key(state, r))
            .collect();
        let restore_focus = self.question_drafts.iter().any(|(key, group)| {
            !keys.contains(key)
                && (group.row_focus.iter().any(|focus| focus.is_focused(window))
                    || group.questions.iter().any(|question| {
                        question.input.focus_handle(cx).is_focused(window)
                            || question
                                .choice_focus
                                .iter()
                                .any(|focus| focus.is_focused(window))
                    }))
        });
        for (key, group) in &mut self.question_drafts {
            if group.pending_receipt.is_some()
                && state.question_answer_acks.iter().any(|ack| {
                    ack.sequence > group.ack_after
                        && format!(
                            "{}:{}:{}",
                            ack.thread_id.as_deref().unwrap_or_default(),
                            if group.turn_bound {
                                ack.turn_id.as_deref().unwrap_or_default()
                            } else {
                                ""
                            },
                            ack.id
                        ) == *key
                })
            {
                if let Some(receipt) = group.pending_receipt.take() {
                    self.question_receipts.push(receipt);
                }
            }
        }
        self.question_drafts.retain(|key, _| keys.contains(key));
        if restore_focus {
            self.input.focus_handle(cx).focus(window, cx);
        }
        for request in &state.questions {
            let key = question_key(state, request);
            if let Some(draft) = self.question_drafts.get_mut(&key) {
                if state.error.is_some() {
                    draft.submitting = false;
                    draft.pending_receipt = None;
                }
                continue;
            }
            let questions = request
                .questions
                .iter()
                .map(|question| {
                    let input = cx.new(|cx| {
                        let placeholder = question.placeholder.clone().unwrap_or_else(|| {
                            if let codex::QuestionKind::Number(number) = &question.kind {
                                format!("e.g. {}", number.min)
                            } else if question.options.is_empty() {
                                "Type your answer…".into()
                            } else {
                                "Type a different answer…".into()
                            }
                        });
                        let mut input = InputState::new(window, cx).placeholder(placeholder);
                        if let codex::QuestionKind::Number(number) = &question.kind {
                            if let Some(value) = number.default_value {
                                input = input.default_value(value.to_string());
                            }
                        }
                        if question.secret {
                            input.masked(true)
                        } else if matches!(question.kind, codex::QuestionKind::Number(_)) {
                            input
                        } else {
                            input.auto_grow(1, 4)
                        }
                    });
                    let subscription = cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify());
                    QuestionDraft {
                        selected: HashSet::new(),
                        choice_focus: (0..question.options.len()
                            + usize::from(question.allow_custom))
                            .map(|_| cx.focus_handle())
                            .collect(),
                        custom: question.options.is_empty(),
                        input,
                        _subscription: subscription,
                    }
                })
                .collect();
            self.question_drafts.insert(
                key,
                QuestionGroupDraft {
                    questions,
                    expanded: request.questions.iter().position(|question| {
                        !matches!(&question.kind, codex::QuestionKind::Number(number) if number.default_value.is_some())
                    }).or(Some(0)),
                    row_focus: request
                        .questions
                        .iter()
                        .map(|_| cx.focus_handle())
                        .collect(),
                    anchor: state.saved.items.len(),
                    ack_after: state
                        .question_answer_acks
                        .last()
                        .map_or(0, |ack| ack.sequence),
                    turn_bound: request.turn_bound(),
                    pending_receipt: None,
                    submitting: false,
                },
            );
        }
    }
    fn question_answers(
        &self,
        key: &str,
        request: &codex::QuestionRequest,
        cx: &App,
    ) -> codex::QuestionAnswers {
        let Some(group) = self.question_drafts.get(key) else {
            return HashMap::new();
        };
        request
            .questions
            .iter()
            .zip(&group.questions)
            .map(|(question, draft)| {
                let mut values: Vec<_> = question
                    .options
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| draft.selected.contains(i))
                    .map(|(_, option)| option.label.clone())
                    .collect();
                if draft.custom {
                    let value = draft.input.read(cx).value();
                    values.push(if question.secret {
                        value.to_string()
                    } else {
                        value.trim().to_owned()
                    });
                }
                (question.id.clone(), values)
            })
            .collect()
    }
    fn select_question_choice(
        &mut self,
        key: &str,
        question_index: usize,
        choice: Option<usize>,
        multiple: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self.question_drafts.get_mut(key) else {
            return;
        };
        if group.submitting {
            return;
        }
        let draft = &mut group.questions[question_index];
        if !multiple {
            draft.selected.clear();
            draft.custom = false;
        }
        if let Some(index) = choice {
            draft.choice_focus[index].focus(window, cx);
            if !draft.selected.insert(index) && multiple {
                draft.selected.remove(&index);
            }
        } else {
            draft.custom = !draft.custom;
            if draft.custom {
                draft.input.focus_handle(cx).focus(window, cx);
            }
        }
        cx.notify();
    }
    fn answer_questions(
        &mut self,
        key: &str,
        id: serde_json::Value,
        cancel: bool,
        cx: &mut Context<Self>,
    ) {
        let state = &self.session.read(cx).state;
        let Some(request) = state
            .questions
            .iter()
            .find(|r| r.id == id && question_key(state, r) == key)
        else {
            return;
        };
        let answers = if cancel {
            None
        } else {
            let answers = self.question_answers(key, request, cx);
            if request.validate(&answers).is_err() {
                return;
            }
            Some(answers)
        };
        let receipt = answers.as_ref().map(|answers| QuestionReceipt {
            anchor: self.question_drafts[key].anchor,
            title: request.title.clone(),
            answers: request
                .questions
                .iter()
                .map(|question| {
                    (
                        question.text.clone(),
                        question_answer_summary(question, answers),
                    )
                })
                .collect(),
        });
        let ack_after = state
            .question_answer_acks
            .last()
            .map_or(0, |ack| ack.sequence);
        let Some(group) = self.question_drafts.get_mut(key) else {
            return;
        };
        if group.submitting {
            return;
        }
        group.pending_receipt = receipt;
        group.ack_after = ack_after;
        group.submitting = true;
        self.session.update(cx, |session, cx| {
            session.state.error = None;
            session.send(Action::AnswerQuestions(id, answers), cx);
        });
        cx.notify();
    }
    fn toggle_question(
        &mut self,
        key: &str,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self.question_drafts.get_mut(key) else {
            return;
        };
        if group.submitting {
            return;
        }
        group.expanded = if group.expanded == Some(index) {
            None
        } else {
            Some(index)
        };
        group.row_focus[index].focus(window, cx);
        cx.notify();
    }

    fn has_question_anchor(&self, anchor: usize) -> bool {
        self.question_drafts
            .values()
            .any(|group| group.anchor == anchor)
            || self
                .question_receipts
                .iter()
                .any(|receipt| receipt.anchor == anchor)
    }

    fn question_blocks_at(
        &mut self,
        anchor: usize,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let mut blocks = Vec::new();
        for (index, receipt) in self
            .question_receipts
            .iter()
            .enumerate()
            .filter(|(_, receipt)| receipt.anchor == anchor)
        {
            let mut group = div()
                .id(("question-receipt", index))
                .debug_selector(move || format!("question-receipt-{index}"))
                .role(gpui::Role::Group)
                .aria_label("Submitted answers")
                .flex_shrink_0()
                .w_full()
                .max_w(rpx(CHAT_MAX_W))
                .min_w_0()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .px(rpx(SPACE_3XL))
                .py(rpx(SPACE_2XL))
                .rounded(rpx(RADIUS_GROUP))
                .border_1()
                .border_color(c::BORDER())
                .bg(c::BG_RAIL())
                .child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child("Answers sent"),
                );
            if let Some(title) = &receipt.title {
                group = group.child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title.clone()),
                );
            }
            for (question, answer) in &receipt.answers {
                group = group.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(rpx(SPACE_XS))
                        .child(
                            div()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child(question.clone()),
                        )
                        .child(div().child(answer.clone())),
                );
            }
            blocks.push(group.into_any_element());
        }
        for (index, request) in state.questions.iter().enumerate() {
            if self
                .question_drafts
                .get(&question_key(state, request))
                .is_some_and(|group| group.anchor == anchor)
            {
                blocks.push(
                    div()
                        .id(("inline-questions", index))
                        .debug_selector(|| "agent-questions".into())
                        .flex_shrink_0()
                        .w_full()
                        .max_w(rpx(CHAT_MAX_W))
                        .min_w_0()
                        .child(self.question_group(state, index, window, cx))
                        .into_any_element(),
                );
            }
        }
        blocks
    }

    fn question_group(
        &mut self,
        state: &codex::Snapshot,
        request_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let request = &state.questions[request_index];
        let key = question_key(state, request);
        let draft = &self.question_drafts[&key];
        let submitting = draft.submitting;
        let answers = self.question_answers(&key, request, cx);
        let answered = request
            .questions
            .iter()
            .filter(|question| question_has_answer(question, &answers))
            .count();
        let validation = request.validate(&answers);
        let ready = !submitting && validation.is_ok();
        let mut group = div()
            .id(("agent-question-group", request_index))
            .role(gpui::Role::Group)
            .aria_label(format!("Questions from {}", state.saved.provider.label()))
            .flex_shrink_0()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .rounded(rpx(RADIUS_GROUP))
            .border_1()
            .border_color(c::BORDER())
            .bg(c::BG_RAIL())
            .overflow_hidden()
            .child(
                div()
                    .px(rpx(SPACE_3XL))
                    .py(rpx(SPACE_2XL))
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::AMBER())
                    .child(format!(
                        "{} needs your input · {answered} of {} answered · draft",
                        state.saved.provider.label(),
                        request.questions.len(),
                    )),
            );
        if let Some(title) = &request.title {
            group = group.child(
                div()
                    .px(rpx(SPACE_3XL))
                    .pb(rpx(SPACE_2XL))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(title.clone()),
            );
        }
        for (question_index, question) in request.questions.iter().enumerate() {
            let question_draft = &draft.questions[question_index];
            let expanded = draft.expanded == Some(question_index);
            let click_key = key.clone();
            let keyboard_key = key.clone();
            group = group.child(
                question_disclosure(
                    gpui::SharedString::from(format!(
                        "question-row-{request_index}-{question_index}"
                    )),
                    format!("{}. {}", question_index + 1, question.text),
                    question_answer_summary(question, &answers),
                    expanded,
                )
                .debug_selector(move || format!("question-row-{request_index}-{question_index}"))
                .track_focus(&draft.row_focus[question_index])
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.toggle_question(&click_key, question_index, window, cx);
                }))
                .on_key_down(cx.listener(
                    move |this, event: &gpui::KeyDownEvent, window, cx| {
                        if action_key(event) {
                            this.toggle_question(&keyboard_key, question_index, window, cx);
                            cx.stop_propagation();
                        }
                    },
                )),
            );
            if !expanded {
                continue;
            }
            let mut field = div()
                .id(gpui::SharedString::from(format!(
                    "question-{request_index}-{question_index}"
                )))
                .role(if question.multiple {
                    gpui::Role::Group
                } else {
                    gpui::Role::RadioGroup
                })
                .aria_label(question.text.clone())
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .px(rpx(SPACE_3XL))
                .pb(rpx(SPACE_3XL))
                .bg(c::FIELD_FILL())
                .when(!question.header.is_empty(), |field| {
                    field.child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(question.header.clone()),
                    )
                });
            if let Some(description) = &question.description {
                field = field.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(description.clone()),
                );
            }
            if let codex::QuestionKind::Number(number) = &question.kind {
                let mut hint = format!("{}–{}", number.min, number.max);
                if let Some(unit) = &number.unit {
                    let _ = write!(hint, " {unit}");
                }
                if let Some(step) = number.step {
                    let _ = write!(hint, " · increment {step}");
                }
                field = field.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(hint),
                );
            }
            if question.multiple {
                field = field.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child("Select all that apply"),
                );
            }
            for (option_index, option) in question.options.iter().enumerate() {
                let click_key = key.clone();
                let keyboard_key = key.clone();
                let multiple = question.multiple;
                let choice_count = question.options.len();
                let option_count = choice_count + usize::from(question.allow_custom);
                field = field.child(
                    question_choice(
                        gpui::SharedString::from(format!(
                            "question-option-{request_index}-{question_index}-{option_index}"
                        )),
                        option.label.clone(),
                        match &option.preview {
                            Some(preview) if !option.description.is_empty() => {
                                format!("{}\n{preview}", option.description)
                            }
                            Some(preview) => preview.clone(),
                            None => option.description.clone(),
                        },
                        multiple,
                        question_draft.selected.contains(&option_index),
                    )
                    .track_focus(&question_draft.choice_focus[option_index])
                    .debug_selector(move || {
                        format!("question-option-{request_index}-{question_index}-{option_index}")
                    })
                    .when(submitting, |row| row.opacity(OPACITY_DISABLED))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_question_choice(
                            &click_key,
                            question_index,
                            Some(option_index),
                            multiple,
                            window,
                            cx,
                        );
                    }))
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            if event.is_held {
                                return;
                            }
                            let next = match event.keystroke.key.as_str() {
                                "enter" | "space" => Some(option_index),
                                "down" | "right" if !multiple => {
                                    Some((option_index + 1) % option_count)
                                }
                                "up" | "left" if !multiple => {
                                    Some((option_index + option_count - 1) % option_count)
                                }
                                _ => None,
                            };
                            if let Some(next) = next {
                                this.select_question_choice(
                                    &keyboard_key,
                                    question_index,
                                    (next < choice_count).then_some(next),
                                    multiple,
                                    window,
                                    cx,
                                );
                                cx.stop_propagation();
                            }
                        },
                    )),
                );
            }
            if question.allow_custom && !question.options.is_empty() {
                let click_key = key.clone();
                let keyboard_key = key.clone();
                let multiple = question.multiple;
                let choice_count = question.options.len();
                field = field.child(
                    question_choice(
                        gpui::SharedString::from(format!(
                            "question-other-{request_index}-{question_index}"
                        )),
                        "Other".into(),
                        String::new(),
                        multiple,
                        question_draft.custom,
                    )
                    .track_focus(&question_draft.choice_focus[question.options.len()])
                    .when(submitting, |row| row.opacity(OPACITY_DISABLED))
                    .debug_selector(move || {
                        format!("question-other-{request_index}-{question_index}")
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_question_choice(
                            &click_key,
                            question_index,
                            None,
                            multiple,
                            window,
                            cx,
                        );
                    }))
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            if event.is_held {
                                return;
                            }
                            let choice = match event.keystroke.key.as_str() {
                                "enter" | "space" => Some(None),
                                "up" | "left" if !multiple => Some(Some(choice_count - 1)),
                                "down" | "right" if !multiple => Some(Some(0)),
                                _ => None,
                            };
                            if let Some(choice) = choice {
                                this.select_question_choice(
                                    &keyboard_key,
                                    question_index,
                                    choice,
                                    multiple,
                                    window,
                                    cx,
                                );
                                cx.stop_propagation();
                            }
                        },
                    )),
                );
            }
            if question_draft.custom {
                let focused = question_draft.input.focus_handle(cx).is_focused(window);
                field = field.child(
                    project_field_well(focused, false)
                        .debug_selector(move || {
                            format!("question-input-{request_index}-{question_index}")
                        })
                        .child(
                            Input::new(&question_draft.input)
                                .disabled(submitting)
                                .aria_label(format!("Answer: {}", question.text))
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false)
                                .text_color(c::FG())
                                .p_0(),
                        ),
                );
            }
            group = group.child(field);
        }
        if let Err(error) = validation {
            group = group.child(
                div()
                    .id(gpui::SharedString::from(format!(
                        "questions-validation-{request_index}"
                    )))
                    .debug_selector(move || format!("questions-validation-{request_index}"))
                    .role(gpui::Role::Alert)
                    .px(rpx(SPACE_3XL))
                    .pt(rpx(SPACE_2XL))
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(error),
            );
        }
        let cancel_key = key.clone();
        let cancel_id = request.id.clone();
        let submit_key = key.clone();
        let submit_id = request.id.clone();
        group = group.child(
            div()
                .px(rpx(SPACE_3XL))
                .py(rpx(SPACE_2XL))
                .border_t_1()
                .border_color(c::BORDER_SOFT())
                .flex()
                .flex_wrap()
                .justify_between()
                .items_center()
                .gap(rpx(SPACE_LG))
                .child(
                    transcript_action(
                        gpui::SharedString::from(format!("questions-cancel-{request_index}")),
                        "Cancel questions",
                        false,
                        window,
                        cx,
                    )
                    .debug_selector(move || format!("questions-cancel-{request_index}"))
                    .disabled(submitting)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.answer_questions(&cancel_key, cancel_id.clone(), true, cx);
                    })),
                )
                .child(
                    transcript_action(
                        gpui::SharedString::from(format!("questions-submit-{request_index}")),
                        if submitting {
                            "Sending…"
                        } else {
                            "Send answers"
                        },
                        true,
                        window,
                        cx,
                    )
                    .debug_selector(move || format!("questions-submit-{request_index}"))
                    .disabled(!ready)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.answer_questions(&submit_key, submit_id.clone(), false, cx);
                    })),
                ),
        );
        group
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grove_core::codex::{Connection, Handle, SavedSession, Snapshot, Status};
    use serde_json::json;
    use std::sync::mpsc;
    #[test]
    fn minimap_pairs_inputs_with_final_or_latest_response_without_crossing_next_input() {
        let items = vec![
            json!({"id":"u1", "type":"userMessage", "content":[{"text":"First input"}]}),
            json!({"type":"agentMessage", "phase":"commentary", "text":"Working"}),
            json!({"type":"commandExecution", "aggregatedOutput":"tool output"}),
            json!({"type":"agentMessage", "phase":"final_answer", "text":"Final result"}),
            json!({"type":"agentMessage", "phase":"commentary", "text":"Late commentary"}),
            json!({"id":"u2", "type":"userMessage", "content":[{"text":"Second input"}]}),
            json!({"type":"agentMessage", "text":"Streaming response"}),
            json!({"id":"u3", "type":"userMessage", "content":[{"type":"image"}]}),
            json!({"id":"u4", "type":"userMessage", "content":[{"text":"Fourth input"}]}),
            json!({"type":"agentMessage", "text":"Fourth response"}),
        ];
        let entries = message_minimap_entries(&items);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.item_index)
                .collect::<Vec<_>>(),
            vec![0, 5, 7, 8]
        );
        assert_eq!(entries[0].prompt, "First input");
        assert_eq!(entries[0].response, "Final result");
        assert_eq!(entries[1].response, "Streaming response");
        assert_eq!(entries[2].prompt, "Image attachment");
        assert_eq!(entries[2].response, "No response yet");
        assert_eq!(entries[3].response, "Fourth response");
        assert!(minimap_excerpt(&"🦀".repeat(500)).ends_with('…'));
    }

    #[gpui::test]
    fn minimap_click_uses_rendered_anchor_and_streaming_does_not_snap_back(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        cx.simulate_resize(gpui::size(gpui::px(1000.0), gpui::px(800.0)));
        cx.update(|_, cx| view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.saved.items = vec![
                    json!({"id":"first", "type":"userMessage", "content":[{"text":"First input"}]}),
                    json!({"id":"tool", "type":"commandExecution", "command":"git status", "status":"completed"}),
                    json!({"id":"first-reply", "type":"agentMessage", "text":"First response.\n\n".repeat(40)}),
                    json!({"id":"second", "type":"userMessage", "content":[{"text":"Second input"}]}),
                    json!({"id":"second-reply", "type":"agentMessage", "text":"Second response.\n\n".repeat(40)}),
                    json!({"id":"third", "type":"userMessage", "content":[{"text":"Third input"}]}),
                    json!({"id":"third-reply", "type":"agentMessage", "text":"Streaming.\n\n".repeat(40)}),
                ];
                cx.notify();
            });
        }));
        draw(cx);
        let rail = cx.debug_bounds("codex-message-minimap").unwrap();
        let first = cx.debug_bounds("codex-minimap-message-0").unwrap();
        let mark = cx.debug_bounds("codex-minimap-message-3").unwrap();
        let last = cx.debug_bounds("codex-minimap-message-5").unwrap();
        let row_height = rpx(SPACE_LG).to_pixels(gpui::px(16.0));
        assert!((mark.top() - first.bottom()).abs() < gpui::px(1.0));
        assert!(mark.size.height <= row_height);
        for bounds in [first, mark, last] {
            assert!((bounds.center().x - rail.center().x).abs() < gpui::px(1.0));
        }
        let stack_center = (first.top() + last.bottom()) / 2.0;
        assert!((stack_center - rail.center().y).abs() < gpui::px(1.0));
        cx.simulate_click(mark.center(), gpui::Modifiers::default());
        draw(cx);
        let transcript = cx.debug_bounds("codex-messages").unwrap();
        let prompt = cx.debug_bounds("codex-message-anchor-3").unwrap();
        assert!(
            (prompt.top() - transcript.top()).abs() < gpui::px(1.0),
            "jump must align the selected user row after tool/header insertion"
        );
        let offset = view.read_with(cx, |view, _| {
            assert!(view.composer_collapsed);
            assert!(!view.smooth_follow);
            view.scroll.offset().y
        });
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Working;
                    session.state.saved.items[6]["text"] = json!("Streaming more.\n\n".repeat(60));
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert_eq!(view.read_with(cx, |view, _| view.scroll.offset().y), offset);
        // Returning to the first turn must resolve against the same rendered list.
        cx.update(|window, cx| {
            window.blur();
            window.focus_next(cx);
        });
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(
            (cx.debug_bounds("codex-message-anchor-0").unwrap().top()
                - cx.debug_bounds("codex-messages").unwrap().top())
            .abs()
                < gpui::px(1.0)
        );
    }

    #[gpui::test]
    fn minimap_keeps_many_turns_reachable_in_narrow_panes(cx: &mut gpui::TestAppContext) {
        let (view, cx, _, _) = fixture(cx);
        cx.simulate_resize(gpui::size(gpui::px(400.0), gpui::px(800.0)));
        cx.update(|_, cx| view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.saved.items = (0..50).flat_map(|index| [
                    json!({"id":format!("user-{index}"), "type":"userMessage", "content":[{"text":format!("Input {index}")}]}),
                    json!({"id":format!("reply-{index}"), "type":"agentMessage", "text":format!("Response {index}")}),
                ]).collect();
                cx.notify();
            });
        }));
        draw(cx);
        let rail = cx.debug_bounds("codex-message-minimap").unwrap();
        let transcript = cx.debug_bounds("codex-messages").unwrap();
        assert!(rail.right() <= transcript.left());
        for selector in [
            "codex-minimap-message-0",
            "codex-minimap-message-20",
            "codex-minimap-message-50",
            "codex-minimap-message-98",
        ] {
            let mark = cx.debug_bounds(selector).unwrap();
            assert!(mark.size.height > gpui::Pixels::ZERO);
            assert!(mark.top() >= rail.top());
            assert!(mark.bottom() <= rail.bottom());
        }
        let mark = cx.debug_bounds("codex-minimap-message-50").unwrap();
        cx.simulate_click(mark.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(
            (cx.debug_bounds("codex-message-anchor-50").unwrap().top()
                - cx.debug_bounds("codex-messages").unwrap().top())
            .abs()
                < gpui::px(1.0)
        );
    }

    #[gpui::test]
    fn minimap_overflow_keeps_first_and_last_turns_reachable(cx: &mut gpui::TestAppContext) {
        let (view, cx, _, _) = fixture(cx);
        cx.simulate_resize(gpui::size(gpui::px(400.0), gpui::px(800.0)));
        cx.update(|_, cx| view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.saved.items = (0..200).map(|index|
                    json!({"id":format!("user-{index}"), "type":"userMessage", "content":[{"text":format!("Input {index}")}]}),
                ).collect();
                cx.notify();
            });
        }));
        draw(cx);
        let rail = cx.debug_bounds("codex-message-minimap").unwrap();
        let first = cx.debug_bounds("codex-minimap-message-0").unwrap();
        assert!(first.top() >= rail.top());
        assert!(first.bottom() <= rail.bottom());
        let scroll_rail = |delta, cx: &mut gpui::VisualTestContext| {
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: rail.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(delta))),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            });
            draw(cx);
        };
        scroll_rail(-100000.0, cx);
        let last = cx.debug_bounds("codex-minimap-message-199").unwrap();
        assert!(last.top() >= rail.top());
        assert!(last.bottom() <= rail.bottom());
        cx.simulate_click(last.center(), gpui::Modifiers::default());
        draw(cx);
        view.read_with(cx, |view, _| {
            assert!(view.scroll.offset().y < gpui::Pixels::ZERO);
        });
        scroll_rail(100000.0, cx);
        let first = cx.debug_bounds("codex-minimap-message-0").unwrap();
        assert!(first.top() >= rail.top());
        assert!(first.bottom() <= rail.bottom());
    }

    #[test]
    fn tool_shimmer_tracks_active_calls_and_claude_thinking() {
        let mut state = snapshot();
        state.status = Status::Working;
        state.saved.provider = codex::Provider::Claude;
        state.saved.items = vec![json!({"type":"reasoning"})];
        assert!(tool_is_running(&state, 0));
        state
            .saved
            .items
            .push(json!({"type":"commandExecution","status":"inProgress"}));
        assert!(!tool_is_running(&state, 0));
        assert!(tool_is_running(&state, 1));
        state.saved.items[1]["status"] = json!("completed");
        assert!(!tool_is_running(&state, 1));
        state.saved.items[1]["status"] = json!("inProgress");
        for status in [
            Status::Waiting,
            Status::Idle,
            Status::Done,
            Status::Interrupted,
        ] {
            state.status = status;
            assert!(!tool_is_running(&state, 1));
        }
        state.status = Status::Working;
        state.saved.items.pop();
        state.saved.items[0]["status"] = json!("completed");
        assert!(!tool_is_running(&state, 0));
        state.saved.items[0]
            .as_object_mut()
            .unwrap()
            .remove("status");
        state.saved.provider = codex::Provider::Codex;
        assert!(!tool_is_running(&state, 0));
    }
    #[gpui::test]
    fn initialized_claude_accepts_first_prompt_before_session_id(cx: &mut gpui::TestAppContext) {
        let (handle, actions) = Handle::channel();
        let (_updates, receive) = mpsc::channel();
        let mut state = snapshot();
        state.saved.provider = codex::Provider::Claude;
        state.saved.thread_id = None;
        state.status = Status::Idle;
        let session = cx.new(|cx| {
            CodexSession::from_connection(
                state,
                Connection {
                    handle,
                    updates: receive,
                },
                cx,
            )
        });
        let result = session.update(cx, |session, cx| {
            session.prompt_with_images("First Claude prompt".into(), Vec::new(), cx)
        });
        assert!(result.is_ok());
        assert!(
            matches!(actions.try_recv().unwrap(), Action::Submit { text, .. } if text == "First Claude prompt")
        );
    }
    #[test]
    fn claude_effort_uses_exact_resolved_catalog_model() {
        let mut state = snapshot();
        state.saved.provider = codex::Provider::Claude;
        state.model = None;
        state.resolved_model = Some("claude-opus-5-5".into());
        state.models = vec![
            json!({"model":"default", "resolvedModel":"claude-opus-5-5", "supportedReasoningEfforts":[{"reasoningEffort":"high"},{"reasoningEffort":"max"}]}),
            json!({"model":"haiku-legacy", "resolvedModel":"claude-haiku-legacy"}),
        ];
        let (choices, _) = CodexView::menu_choices(&state, ComposerMenu::Effort);
        assert!(choices.iter().any(|(value, _)| value == "high"));
        assert!(choices.iter().any(|(value, _)| value == "max"));
        state.model = Some("haiku-legacy".into());
        let (choices, _) = CodexView::menu_choices(&state, ComposerMenu::Effort);
        assert_eq!(
            choices,
            vec![("__configured".into(), "Restore configured defaults".into())]
        );
        state.model = Some("unknown-custom-model".into());
        assert!(effective_model(&state).is_none());
    }
    #[test]
    fn claude_permissions_use_claude_modes_without_codex_sandbox_claims() {
        let mut state = snapshot();
        state.saved.provider = codex::Provider::Claude;
        let (choices, _) = CodexView::menu_choices(&state, ComposerMenu::Access);
        assert!(choices.iter().any(|(value, _)| value == "default"));
        assert!(choices
            .iter()
            .any(|(value, _)| value == "bypassPermissions"));
        assert!(!choices
            .iter()
            .any(|(value, label)| value == "workspace-write" || label.contains("sandbox")));
    }
    fn test_image(format: image::ImageFormat) -> Vec<u8> {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(2, 2)
            .write_to(&mut bytes, format)
            .unwrap();
        bytes.into_inner()
    }
    fn snapshot() -> Snapshot {
        let mut state = Snapshot::new(SavedSession {
            provider: grove_core::codex::Provider::Codex,
            composer: grove_core::codex::ComposerSettings::default(),
            context: None,
            work_periods: Vec::new(),
            key: "native-ui-test".into(),
            project: "Demo".into(),
            cwd: "/unused".into(),
            label: "Codex".into(),
            title: None,
            roots: Vec::new(),
            thread_id: Some("thread-ui".into()),
            items: Vec::new(),
            draft: String::new(),
            draft_images: Vec::new(),
            config_args: Vec::new(),
            task_id: None,
        });
        state.status = Status::Idle;
        state
    }
    fn fixture(
        cx: &mut gpui::TestAppContext,
    ) -> (
        Entity<CodexView>,
        &mut gpui::VisualTestContext,
        mpsc::Receiver<Action>,
        mpsc::Sender<Snapshot>,
    ) {
        cx.update(gpui_component::init);
        let (handle, actions) = Handle::channel();
        let (updates, receive) = mpsc::channel();
        let connection = Connection {
            handle,
            updates: receive,
        };
        let mut view = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let session = cx.new(|cx| CodexSession::from_connection(snapshot(), connection, cx));
            let chat = cx.new(|cx| CodexView::new(session, window, cx));
            view = Some(chat.clone());
            gpui_component::Root::new(chat, window, cx).bordered(false)
        });
        (view.unwrap(), cx, actions, updates)
    }
    fn draw(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    #[gpui::test]
    fn provider_connection_centers_then_reveals_launch_and_failure_recovery(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        cx.simulate_resize(gpui::size(gpui::px(1000.0), gpui::px(800.0)));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("Preserved request", window, cx);
                });
                view.session.update(cx, |session, cx| {
                    session.state.saved.thread_id = None;
                    session.state.status = Status::Starting;
                    cx.notify();
                });
            });
        });
        draw(cx);
        let connection = cx.debug_bounds("codex-provider-connection").unwrap();
        assert!((f32::from(connection.center().x) - 500.0).abs() < 1.0);
        assert!((f32::from(connection.center().y) - 400.0).abs() < 1.0);
        for selector in [
            "codex-composer",
            "codex-empty",
            "codex-launch-starters",
            "codex-live-activity",
        ] {
            assert!(cx.debug_bounds(selector).is_none(), "{selector}");
        }
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Idle;
                cx.notify();
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-provider-connection").is_none());
        assert!(cx.debug_bounds("codex-composer").is_some());
        assert!(cx.debug_bounds("codex-launch-starters").is_some());
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Disconnected;
                session.state.error = Some("Provider unavailable".into());
                cx.notify();
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-provider-connection").is_none());
        assert!(cx.debug_bounds("codex-composer").is_some());
        assert!(cx.debug_bounds("codex-error").is_some());
        let reconnect = cx.debug_bounds("codex-reconnect").unwrap();
        cx.simulate_click(reconnect.center(), gpui::Modifiers::default());
        assert!(actions
            .try_iter()
            .any(|action| matches!(action, Action::Reconnect)));
        view.read_with(cx, |view, cx| {
            assert_eq!(view.input.read(cx).value().as_ref(), "Preserved request");
        });
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Starting;
                session.state.error = None;
                session.state.saved.items.push(json!({"type":"userMessage", "id":"existing", "content":[{"type":"text", "text":"Earlier request"}]}));
                cx.notify();
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-provider-connection").is_none());
        assert!(cx.debug_bounds("codex-composer").is_some());
    }

    #[gpui::test]
    fn focused_launch_starters_preserve_draft_and_never_submit(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, _) = fixture(cx);
        draw(cx);
        assert!(cx.debug_bounds("codex-empty").is_some());
        let starter = cx.debug_bounds("codex-starter-review").unwrap();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("My existing request", window, cx);
                });
            });
        });
        cx.simulate_click(starter.center(), gpui::Modifiers::default());
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.input.read(cx).value().as_ref(),
                "My existing request\n\nReview my uncommitted changes for bugs and regressions."
            );
            assert!(view.submitted.is_none());
        });
        let sent: Vec<_> = actions.try_iter().collect();
        assert!(!sent.is_empty());
        assert!(sent.iter().all(|action| matches!(action, Action::Draft(_))));
        cx.update(|window, cx| {
            assert!(view.read(cx).input.focus_handle(cx).is_focused(window));
            let session = view.read(cx).session.clone();
            session.update(cx, |session, cx| {
                session.state.saved.items.push(json!({"type":"userMessage", "id":"first", "content":[{"type":"text", "text":"Hello"}]}));
                cx.notify();
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-empty").is_none());
        assert!(cx.debug_bounds("codex-starter-review").is_none());
        assert!(cx.debug_bounds("codex-composer").is_some());
    }

    #[gpui::test]
    fn focused_launch_wraps_starters_inside_narrow_panes(cx: &mut gpui::TestAppContext) {
        let (_view, cx, _, _) = fixture(cx);
        for width in [400.0, 1280.0] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.0)));
            draw(cx);
            let composer = cx.debug_bounds("codex-composer").unwrap();
            let first = cx.debug_bounds("codex-starter-understand").unwrap();
            let last = cx.debug_bounds("codex-starter-review").unwrap();
            assert!(first.top() >= composer.bottom());
            assert!(first.left() >= composer.left());
            assert!(last.right() <= composer.right());
            if width < 600.0 {
                assert!(last.top() > first.top());
            } else {
                assert_eq!(last.top(), first.top());
                assert_eq!(
                    cx.debug_bounds("codex-starter-review-title").unwrap().top(),
                    cx.debug_bounds("codex-starter-understand-title")
                        .unwrap()
                        .top()
                );
            }
        }
    }

    #[gpui::test]
    fn queue_editor_waits_for_hold_and_restores_unrelated_draft(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("Unrelated draft", window, cx);
                });
                view.images = vec!["draft-image".into()];
                view.session.update(cx, |session, _| {
                    session.state.queued_prompts.push(codex::QueuedPrompt {
                        id: 81,
                        text: "Queued original".into(),
                        images: vec!["queue-image".into()],
                    });
                });
                view.begin_queue_edit(81, cx);
                assert_eq!(view.input.read(cx).value().as_ref(), "Unrelated draft");
                assert!(!view.queue_editor.as_ref().unwrap().loaded);
            });
        });
        assert!(actions
            .try_iter()
            .any(|action| matches!(action, Action::BeginQueuedEdit(81))));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("Latest draft before grant", window, cx);
                });
                view.images = vec!["latest-draft-image".into()];
            });
        });
        view.update(cx, |view, cx| {
            view.session
                .update(cx, |session, _| session.state.queue_edit = Some(81));
        });
        draw(cx);
        let cancel = cx.debug_bounds("codex-cancel-queue-edit").unwrap();
        let save = cx.debug_bounds("codex-save-queue-edit").unwrap();
        assert_eq!(cancel.size, save.size);
        assert_eq!(cancel.size.width, gpui::px(CHROME_CONTROL_H));
        assert_eq!(cancel.size.height, gpui::px(CHROME_CONTROL_H));
        view.read_with(cx, |view, cx| {
            assert_eq!(view.input.read(cx).value().as_ref(), "Queued original");
            assert_eq!(view.images, vec!["queue-image"]);
        });
        cx.update(|window, cx| view.update(cx, |view, cx| view.cancel_queue_edit(window, cx)));
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.input.read(cx).value().as_ref(),
                "Latest draft before grant"
            );
            assert_eq!(view.images, vec!["latest-draft-image"]);
            assert!(view.queue_editor.is_none());
        });
        assert!(actions
            .try_iter()
            .any(|action| matches!(action, Action::CancelQueuedEdit(81))));
    }
    #[gpui::test]
    fn queue_dispatch_race_keeps_original_composer(cx: &mut gpui::TestAppContext) {
        let (view, cx, _, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input
                    .update(cx, |input, cx| input.set_value("Keep me", window, cx));
                view.session.update(cx, |session, _| {
                    session.state.queued_prompts.push(codex::QueuedPrompt {
                        id: 82,
                        text: "Already sending".into(),
                        images: vec![],
                    });
                });
                view.begin_queue_edit(82, cx);
                view.session
                    .update(cx, |session, _| session.state.queue_sending.push(82));
            });
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.input.read(cx).value().as_ref(), "Keep me");
            assert!(view.queue_editor.is_none());
        });
    }
    #[gpui::test]
    fn queue_keyboard_traverses_waiting_rows_and_returns_to_input(cx: &mut gpui::TestAppContext) {
        let (view, cx, _, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, _| {
                    session.state.queued_prompts = (1..=3)
                        .map(|id| codex::QueuedPrompt {
                            id,
                            text: id.to_string(),
                            images: vec![],
                        })
                        .collect();
                    session.state.queue_sending.push(1);
                });
                view.input.focus_handle(cx).focus(window, cx);
                assert!(view.move_queue_selection(true, window, cx));
                assert_eq!(view.queue_selected, Some(3));
                assert!(view.move_queue_selection(true, window, cx));
                assert_eq!(view.queue_selected, Some(2));
                assert!(view.move_queue_selection(false, window, cx));
                assert_eq!(view.queue_selected, Some(3));
                assert!(view.move_queue_selection(false, window, cx));
                assert_eq!(view.queue_selected, None);
            });
        });
    }
    #[gpui::test]
    fn queue_exit_rows_expire_and_reduced_motion_removes_immediately(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        view.update(cx, |view, cx| {
            let mut state = snapshot();
            state.queued_prompts.push(codex::QueuedPrompt {
                id: 1,
                text: "One".into(),
                images: vec![],
            });
            view.sync_queue_motion(&state, cx);
            state.queued_prompts.clear();
            view.sync_queue_motion(&state, cx);
            if cx.reduce_motion() {
                assert!(view.queue_motion_rows.is_empty());
            } else {
                assert_eq!(view.queue_motion_rows.len(), 1);
                assert!(view.queue_motion_rows[0].exiting);
                view.queue_motion_rows[0].started = std::time::Instant::now()
                    .checked_sub(std::time::Duration::from_millis(MOTION_BASE_MS + 1))
                    .unwrap();
                view.sync_queue_motion(&state, cx);
                assert!(view.queue_motion_rows.is_empty());
            }
        });
    }
    #[gpui::test]
    fn tab_accepts_file_at_caret_and_preserves_surrounding_text(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.file_query = "src".into();
                view.file_lookup = Some("src".into());
                view.file_suggestions = vec!["src/main.rs".into()];
                view.input.update(cx, |input, cx| {
                    input.set_value("Read @src later", window, cx);
                    input.set_selected_range(9..9, cx);
                });
                view.input.focus_handle(cx).focus(window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("tab");
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.input.read(cx).value().as_ref(),
                "Read @src/main.rs later"
            );
            assert_eq!(view.input.read(cx).cursor(), 18);
            assert_eq!(view.picked_tokens.len(), 1);
            assert_eq!(view.picked_tokens[0].decoration.get_ranges(cx), vec![5..17]);
        });
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::Submit { .. })));
    }

    #[gpui::test]
    fn tab_accepts_skill_and_marker_follows_edits_and_clears_on_change(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, _| {
                    session.state.skills = vec![json!({"name":"review","enabled":true})];
                });
                view.input.update(cx, |input, cx| {
                    input.set_value("Use $rev", window, cx);
                    input.set_selected_range(8..8, cx);
                });
                view.input.focus_handle(cx).focus(window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("tab");
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.input.read(cx).value().as_ref(), "Use $review ");
            assert_eq!(view.picked_tokens.len(), 1);
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_selected_range(0..0, cx);
                    input.insert("Please ", window, cx);
                });
            });
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.picked_tokens[0].decoration.get_ranges(cx),
                vec![11..18]
            );
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_selected_range(15..15, cx);
                    input.insert("x", window, cx);
                });
            });
        });
        draw(cx);
        view.read_with(cx, |view, _| assert!(view.picked_tokens.is_empty()));
    }

    #[gpui::test]
    fn tab_accepts_slash_command_without_executing_until_enter(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("/acc", window, cx);
                    input.set_selected_range(4..4, cx);
                });
                view.input.focus_handle(cx).focus(window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("tab");
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.input.read(cx).value().as_ref(), "/access ");
            assert!(view.menu.is_none());
            assert_eq!(view.picked_tokens.len(), 1);
        });
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::Configure { .. } | Action::Submit { .. })));
        cx.simulate_keystrokes("enter");
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert!(view.menu == Some(ComposerMenu::Access));
            assert!(view.input.read(cx).value().is_empty());
        });
    }

    #[gpui::test]
    fn composer_choices_dispatch_configuration_and_commands_without_prompts(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.menu = Some(ComposerMenu::Model);
                view.choose_option("supported-model", window, cx);
                view.input
                    .update(cx, |input, cx| input.set_value("/access", window, cx));
                view.submit(window, cx);
                assert!(view.menu == Some(ComposerMenu::Access));
                assert!(view.input.read(cx).value().is_empty());
                view.choose_option("read-only", window, cx);
                view.session.update(cx, |session, _| {
                    session.state.status = Status::Working;
                    session.state.turn_id = Some("command-turn".into());
                });
                view.input
                    .update(cx, |input, cx| input.set_value("/stop", window, cx));
                view.submit(window, cx);
            });
        });
        let actions = actions.try_iter().collect::<Vec<_>>();
        assert!(actions.iter().any(|action| matches!(action, Action::Configure { model: Some(model), .. } if model == "supported-model")));
        assert!(actions.iter().any(|action| matches!(action, Action::Configure { access: Some(access), .. } if access == "read-only")));
        assert!(actions
            .iter()
            .any(|action| matches!(action, Action::Interrupt)));
        assert!(!actions.iter().any(|action| matches!(
            action,
            Action::Submit { .. } | Action::SubmitWithMode { .. }
        )));
    }
    #[gpui::test]
    fn sent_bubble_entrance_is_local_once_and_respects_reduced_motion(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                let mut state = snapshot();
                state.saved.items =
                    vec![json!({"id":"history","type":"userMessage","content":[{"text":"hello"}]})];
                view.sync_bubble_entrances(&state, false, cx.background_executor().now());
                assert!(view.animating_bubbles.is_empty());
                view.bubble_entrances.push(BubbleEntrance {
                    submission: 1,
                    existing: HashSet::from(["history".into()]),
                    thread: state.saved.thread_id.clone(),
                    text: "hello".into(),
                });
                view.sync_bubble_entrances(&state, false, cx.background_executor().now());
                assert!(view.animating_bubbles.is_empty());
                state
                    .saved
                    .items
                    .push(json!({"id":"fresh","type":"userMessage","content":[{"text":"hello"}]}));
                view.sync_bubble_entrances(&state, false, cx.background_executor().now());
                assert!(view.bubble_entrances.is_empty());
                let clock = view.animating_bubbles["fresh"].1.clone();
                view.sync_bubble_entrances(&state, false, cx.background_executor().now());
                assert!(Rc::ptr_eq(&clock, &view.animating_bubbles["fresh"].1));
                clock.set(true);
                view.sync_bubble_entrances(&state, false, cx.background_executor().now());
                assert!(view.animating_bubbles.is_empty());
                view.sync_bubble_entrances(&state, false, cx.background_executor().now());
                assert!(view.animating_bubbles.is_empty());
                view.bubble_entrances.push(BubbleEntrance {
                    submission: 2,
                    existing: HashSet::new(),
                    thread: state.saved.thread_id.clone(),
                    text: "hello".into(),
                });
                view.sync_bubble_entrances(&state, true, cx.background_executor().now());
                assert!(view.bubble_entrances.is_empty());
                assert!(view.animating_bubbles.is_empty());
                view.sync_bubble_entrances(&state, false, cx.background_executor().now());
                assert!(view.animating_bubbles.is_empty());
            });
        });
    }
    #[gpui::test]
    fn sending_bubble_rises_and_settles_without_replay(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, updates) = fixture(cx);
        draw(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input
                    .update(cx, |input, cx| input.set_value("hello", window, cx));
                view.input.focus_handle(cx).focus(window, cx);
            });
        });
        draw(cx);
        cx.update(|window, cx| view.update(cx, |view, cx| view.submit(window, cx)));
        draw(cx);
        assert!(actions
            .try_iter()
            .any(|action| matches!(action, Action::Submit { text, .. } if text == "hello")));
        let mut state = snapshot();
        state.accepted_submission = view.read_with(cx, |view, _| view.submitted_ack);
        state
            .saved
            .items
            .push(json!({"id":"fresh","type":"userMessage","content":[{"text":"hello"}]}));
        updates.send(state.clone()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        let entering = cx.debug_bounds("codex-message-row-0").unwrap();
        assert!(view.read_with(cx, |view, _| view.animating_bubbles.contains_key("fresh")));
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(MOTION_SLOW_MS));
        draw(cx);
        // A normal redraw consumes the finished clock without creating a new one.
        view.update(cx, |_, cx| cx.notify());
        draw(cx);
        let settled = cx.debug_bounds("codex-message-row-0").unwrap();
        assert!(entering.top() > settled.top());
        updates.send(state).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert_eq!(settled, cx.debug_bounds("codex-message-row-0").unwrap());
        assert!(view.read_with(cx, |view, _| view.animating_bubbles.is_empty()));
    }
    #[gpui::test]
    fn user_messages_are_narrower_and_right_aligned(cx: &mut gpui::TestAppContext) {
        let (_view, cx, _, updates) = fixture(cx);
        let mut state = snapshot();
        state.saved.items = vec![
            json!({"id":"user","type":"userMessage","content":[{"type":"text","text":"Please inspect this worktree."}]}),
            json!({"id":"agent","type":"agentMessage","text":"I can inspect this worktree."}),
        ];
        updates.send(state).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        let user = cx.debug_bounds("codex-message-row-0").unwrap();
        let agent = cx.debug_bounds("codex-message-row-1").unwrap();
        assert!(user.size.width < agent.size.width);
        assert!(user.origin.x > agent.origin.x);
        assert_eq!(user.right(), agent.right());
    }
    #[gpui::test]
    fn context_meter_mounts_left_of_attach_only_when_usage_is_known(cx: &mut gpui::TestAppContext) {
        let (_view, cx, _, updates) = fixture(cx);
        draw(cx);
        assert!(cx.debug_bounds("codex-attach").is_some());
        assert!(cx.debug_bounds("codex-context-meter").is_none());
        let mut state = snapshot();
        state.saved.context = Some(codex::ContextUsage {
            used: 76_000,
            window: 200_000,
        });
        updates.send(state).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        let meter = cx.debug_bounds("codex-context-meter").unwrap();
        let attach = cx.debug_bounds("codex-attach").unwrap();
        assert!(meter.right() <= attach.left());
        assert!((meter.center().y - attach.center().y).abs() < gpui::px(1.0));
        let mut state = snapshot();
        state.saved.context = Some(codex::ContextUsage { used: 1, window: 0 });
        updates.send(state).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(cx.debug_bounds("codex-context-meter").is_none());
    }
    #[test]
    fn context_meter_turns_amber_at_the_threshold_and_details_tokens() {
        let usage = |used| codex::ContextUsage {
            used,
            window: 200_000,
        };
        assert_eq!(context_tone(usage(76_000)), (38, c::FG_MUTE()));
        assert_eq!(context_tone(usage(158_000)).1, c::FG_MUTE());
        assert_eq!(context_tone(usage(159_000)), (80, c::AMBER()));
        assert_eq!(context_tone(usage(250_000)), (100, c::AMBER()));
        assert_eq!(context_detail(usage(76_000)), "76k / 200k tokens");
    }
    #[gpui::test]
    fn compaction_divider_renders_between_tool_groups_and_waits_while_running(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, updates) = fixture(cx);
        view.update(cx, |view, _| {
            view.expanded.insert("work-0".into());
        });
        let mut state = snapshot();
        state.saved.items = vec![
            json!({"id":"a","type":"commandExecution","command":"pwd","status":"completed"}),
            json!({"id":"c","type":codex::COMPACTION_ITEM,"status":"inProgress","trigger":"auto","preTokens":182_000,"postTokens":null}),
        ];
        updates.send(state.clone()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(cx.debug_bounds("codex-compaction-1").is_none());
        state.saved.items[1]["status"] = json!("completed");
        state.saved.items[1]["postTokens"] = json!(24_000);
        state
            .saved
            .items
            .push(json!({"id":"b","type":"commandExecution","command":"ls","status":"completed"}));
        updates.send(state.clone()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        let divider = cx.debug_bounds("codex-compaction-1").unwrap();
        let before = cx.debug_bounds("codex-tool-group-0").unwrap();
        let after = cx.debug_bounds("codex-tool-group-2").unwrap();
        assert!(before.bottom() <= divider.top() && divider.bottom() <= after.top());
        assert_eq!(
            codex::item_text(&state.saved.items[1]),
            "Context compacted · auto · 182k → 24k"
        );
        assert!(!is_tool_item(&state.saved.items[1]));
    }
    #[gpui::test]
    fn compaction_shows_the_live_activity_line_even_after_tools(cx: &mut gpui::TestAppContext) {
        let (_view, cx, _, updates) = fixture(cx);
        let mut state = snapshot();
        state.status = Status::Working;
        state.turn_id = Some("turn".into());
        state.saved.items =
            vec![json!({"id":"a","type":"commandExecution","command":"pwd","status":"completed"})];
        assert_eq!(live_activity(&state), None);
        state
            .saved
            .items
            .push(json!({"type":"agentMessage","phase":"commentary","text":"Checking results"}));
        assert_eq!(live_activity(&state), None);
        updates.send(state.clone()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(cx.debug_bounds("codex-toggle-tool-0").is_some());
        assert!(cx.debug_bounds("codex-live-activity").is_none());
        state.compacting = true;
        assert_eq!(
            live_activity(&state).as_deref(),
            Some("Compacting context…")
        );
        updates.send(state.clone()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(cx.debug_bounds("codex-live-activity").is_some());
        // A Codex compaction item that has not completed also counts.
        state.compacting = false;
        state
            .saved
            .items
            .push(json!({"id":"c","type":codex::COMPACTION_ITEM,"status":"inProgress"}));
        assert_eq!(
            live_activity(&state).as_deref(),
            Some("Compacting context…")
        );
    }
    #[gpui::test]
    fn clear_waits_for_actor_ack_and_preserves_rejected_draft(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, _) = fixture(cx);
        for (status, compacting, expected) in [
            (Status::Starting, false, codex::CLEAR_BUSY),
            (Status::Working, false, codex::CLEAR_BUSY),
            (Status::Idle, true, codex::CLEAR_BUSY),
            (Status::Disconnected, false, codex::CLEAR_DISCONNECTED),
        ] {
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, _| {
                        session.state.status = status;
                        session.state.compacting = compacting;
                    });
                    view.images = vec!["attached.png".into()];
                    view.input
                        .update(cx, |input, cx| input.set_value("/clear", window, cx));
                    view.submit(window, cx);
                    assert_eq!(view.input.read(cx).value().as_ref(), "/clear");
                    assert_eq!(view.images, vec!["attached.png"]);
                    assert_eq!(view.session.read(cx).state.error.as_deref(), Some(expected));
                });
            });
        }
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::ClearConversation)));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, _| {
                    session.state.status = Status::Idle;
                    session.state.compacting = false;
                });
                view.submit(window, cx);
                assert_eq!(view.input.read(cx).value().as_ref(), "/clear");
                assert_eq!(view.images, vec!["attached.png"]);
            });
        });
        assert_eq!(
            actions
                .try_iter()
                .filter(|action| matches!(action, Action::ClearConversation))
                .count(),
            1
        );
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.cleared_conversations += 1;
                cx.notify();
            });
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert!(view.input.read(cx).value().is_empty());
            assert!(view.images.is_empty());
        });
    }

    #[gpui::test]
    fn clear_in_queue_and_steer_modes_preserves_edits_after_dispatch(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        for mode in [SubmissionMode::Queue, SubmissionMode::Steer] {
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.input
                        .update(cx, |input, cx| input.set_value("/clear", window, cx));
                    view.images = vec!["old.png".into()];
                    view.submit_mode(mode, window, cx);
                    // New composer edits must survive an asynchronous provider reset.
                    view.input
                        .update(cx, |input, cx| input.set_value("new draft", window, cx));
                    view.images = vec!["new.png".into()];
                    view.session.update(cx, |session, cx| {
                        session.state.cleared_conversations += 1;
                        cx.notify();
                    });
                });
            });
            draw(cx);
            view.read_with(cx, |view, cx| {
                assert_eq!(view.input.read(cx).value().as_ref(), "new draft");
                assert_eq!(view.images, vec!["new.png"]);
                assert!(view.pending_clear.is_none());
            });
        }
        let sent = actions.try_iter().collect::<Vec<_>>();
        assert_eq!(
            sent.iter()
                .filter(|action| matches!(action, Action::ClearConversation))
                .count(),
            2
        );
        assert!(!sent.iter().any(|action| matches!(
            action,
            Action::Submit { .. } | Action::SubmitWithMode { .. }
        )));
    }

    #[gpui::test]
    fn host_arguments_are_rejected_in_every_submission_mode(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, _) = fixture(cx);
        for command in [
            "model", "effort", "access", "stop", "queue", "compact", "clear",
        ] {
            for mode in [
                SubmissionMode::Send,
                SubmissionMode::Queue,
                SubmissionMode::Steer,
            ] {
                cx.update(|window, cx| {
                    view.update(cx, |view, cx| {
                        let draft = format!("/{command} unexpected argument");
                        view.input
                            .update(cx, |input, cx| input.set_value(draft.clone(), window, cx));
                        view.images = vec!["attached.png".into()];
                        view.submit_mode(mode, window, cx);
                        assert_eq!(view.input.read(cx).value().as_ref(), draft);
                        assert_eq!(view.images, vec!["attached.png"]);
                        assert!(view
                            .session
                            .read(cx)
                            .state
                            .error
                            .as_deref()
                            .unwrap()
                            .contains("does not accept arguments"));
                        assert!(view.menu.is_none());
                    });
                });
            }
        }
        assert!(!actions.try_iter().any(|action| matches!(
            action,
            Action::Submit { .. }
                | Action::SubmitWithMode { .. }
                | Action::ClearConversation
                | Action::Compact
                | Action::Interrupt
                | Action::ResumeQueue
        )));
    }

    #[gpui::test]
    fn queue_entry_routes_host_commands_and_reports_unavailable_actions(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                for (draft, error) in [
                    ("/stop", "There is no active turn"),
                    ("/queue", "There are no queued prompts"),
                ] {
                    view.input
                        .update(cx, |input, cx| input.set_value(draft, window, cx));
                    view.submit_mode(SubmissionMode::Queue, window, cx);
                    assert_eq!(view.input.read(cx).value().as_ref(), draft);
                    assert!(view
                        .session
                        .read(cx)
                        .state
                        .error
                        .as_deref()
                        .unwrap()
                        .contains(error));
                }
                view.input
                    .update(cx, |input, cx| input.set_value("/access", window, cx));
                view.submit_mode(SubmissionMode::Queue, window, cx);
                assert!(view.menu == Some(ComposerMenu::Access));
                assert!(view.input.read(cx).value().is_empty());
            });
        });
        assert!(!actions.try_iter().any(|action| matches!(
            action,
            Action::Submit { .. } | Action::SubmitWithMode { .. }
        )));
    }

    #[gpui::test]
    fn claude_provider_commands_pass_through_with_arguments(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, _| {
                    session.state.saved.provider = codex::Provider::Claude;
                });
                view.input.update(cx, |input, cx| {
                    input.set_value("/my-skill useful instructions", window, cx);
                });
                view.submit(window, cx);
            });
        });
        assert!(actions.try_iter().any(|action| matches!(action, Action::Submit { text, .. } if text == "/my-skill useful instructions")));
        let mut claude = snapshot();
        claude.saved.provider = codex::Provider::Claude;
        claude.skills = vec![
            json!({"name":"disabled","enabled":false}),
            json!({"name":"enabled"}),
            json!({"name":"clear"}),
        ];
        let choices = slash_choices(&claude, "");
        assert!(!choices.iter().any(|(value, _)| value == "/disabled"));
        assert!(choices.iter().any(|(value, _)| value == "/enabled"));
        assert_eq!(
            choices
                .iter()
                .filter(|(value, _)| value == "/clear")
                .count(),
            1
        );
    }

    #[gpui::test]
    fn compact_command_is_hosted_once_and_rejected_inline_while_busy(
        cx: &mut gpui::TestAppContext,
    ) {
        let mut claude = snapshot();
        claude.saved.provider = codex::Provider::Claude;
        claude.skills = vec![
            json!({"name":"compact","description":"Claude command"}),
            json!({"name":"context","description":"Claude command"}),
        ];
        let commands = slash_choices(&claude, "comp");
        assert_eq!(
            commands
                .iter()
                .filter(|(value, _)| value == "/compact")
                .count(),
            1
        );
        assert!(commands[0].1.contains("Compact conversation context"));
        let (view, cx, actions, _) = fixture(cx);
        let run = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.input
                        .update(cx, |input, cx| input.set_value("/compact", window, cx));
                    view.submit(window, cx);
                });
            });
        };
        run(cx);
        assert_eq!(
            view.read_with(cx, |view, cx| view.session.read(cx).state.error.clone()),
            Some(codex::NOTHING_TO_COMPACT.into())
        );
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, _| {
                session.state.saved.items =
                    vec![json!({"id":"a","type":"agentMessage","text":"Done"})];
                session.state.status = Status::Working;
                session.state.turn_id = Some("turn".into());
            });
        });
        run(cx);
        assert_eq!(
            view.read_with(cx, |view, cx| view.session.read(cx).state.error.clone()),
            Some(codex::COMPACT_BUSY.into())
        );
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::Compact)));
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, _| {
                session.state.status = Status::Done;
                session.state.turn_id = None;
            });
        });
        run(cx);
        let sent = actions.try_iter().collect::<Vec<_>>();
        assert!(sent.iter().any(|action| matches!(action, Action::Compact)));
        assert!(!sent.iter().any(|action| matches!(
            action,
            Action::Submit { .. } | Action::SubmitWithMode { .. } | Action::Prompt(_)
        )));
    }
    #[test]
    fn mention_triggers_require_a_token_boundary_and_commands_start_the_draft() {
        assert_eq!(trailing_trigger("look at @src/"), Some(('@', "src/")));
        assert_eq!(trailing_trigger("use $review"), Some(('$', "review")));
        assert_eq!(trailing_trigger("/mod"), Some(('/', "mod")));
        assert_eq!(trailing_trigger("email@example.com"), None);
        assert_eq!(trailing_trigger("inspect /tmp"), None);
        assert_eq!(trailing_trigger("@src/ "), None);
    }

    #[gpui::test]
    fn completed_work_hides_commentary_and_keeps_answer_outside(cx: &mut gpui::TestAppContext) {
        let (view, cx, _, _) = fixture(cx);
        cx.update(|_, cx| view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Done;
                session.state.saved.items = vec![
                    json!({"id":"prompt", "type":"userMessage", "content":[{"text":"Check status"}]}),
                    json!({"id":"comment", "type":"agentMessage", "phase":"commentary", "text":"Checking the repository."}),
                    json!({"id":"command", "type":"commandExecution", "command":"git status", "status":"completed", "aggregatedOutput":"output\n".repeat(100)}),
                    json!({"id":"answer", "type":"agentMessage", "phase":"final_answer", "text":"The working tree is clean."}),
                ];
                cx.notify();
            });
        }));
        draw(cx);
        assert!(cx.debug_bounds("codex-message-body-1").is_none());
        assert!(cx.debug_bounds("codex-toggle-tool-2").is_none());
        assert!(cx.debug_bounds("codex-message-body-3").is_some());
        let header = cx.debug_bounds("codex-work-header-1").unwrap();
        cx.simulate_click(header.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("codex-message-body-1").is_some());
        assert!(cx.debug_bounds("codex-toggle-tool-2").is_some());
        assert!(cx.debug_bounds("codex-message-body-3").is_some());
        let tool = cx.debug_bounds("codex-toggle-tool-2").unwrap();
        cx.simulate_click(tool.center(), gpui::Modifiers::default());
        draw(cx);
        let detail = cx.debug_bounds("codex-tool-detail-2").unwrap();
        assert!(
            detail.size.height
                <= cx.update(|window, _| rpx(TOOL_DETAIL_MAX_H).to_pixels(window.rem_size()))
        );
        assert!(cx.debug_bounds("codex-message-body-3").is_some());
        cx.simulate_resize(gpui::size(gpui::px(400.0), gpui::px(800.0)));
        draw(cx);
        let detail = cx.debug_bounds("codex-tool-detail-2").unwrap();
        assert!(
            detail.size.height
                <= cx.update(|window, _| rpx(TOOL_DETAIL_MAX_H).to_pixels(window.rem_size()))
        );
        let header = cx.debug_bounds("codex-work-header-1").unwrap();
        cx.simulate_click(header.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("codex-message-body-1").is_none());
        assert!(cx.debug_bounds("codex-tool-detail-2").is_none());
        assert!(cx.debug_bounds("codex-message-body-3").is_some());
    }

    #[test]
    fn work_commentary_preserves_explicit_answers_and_legacy_last_response() {
        let mut state = snapshot();
        state.saved.items = vec![
            json!({"type":"userMessage"}),
            json!({"type":"agentMessage", "text":"I will inspect"}),
            json!({"type":"commandExecution"}),
            json!({"type":"agentMessage", "phase":"final_answer", "text":"Done"}),
            json!({"type":"agentMessage", "phase":"commentary", "text":"Late progress"}),
            json!({"type":"userMessage"}),
            json!({"type":"agentMessage", "text":"Legacy final"}),
        ];
        assert!(work_commentary(&state, 1));
        assert!(!work_commentary(&state, 3));
        assert!(work_commentary(&state, 4));
        assert!(!work_commentary(&state, 6));
    }

    #[gpui::test]
    fn completed_work_disclosure_contains_tools_and_active_call_stays_visible(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.expanded.insert("work-0".into());
                view.expanded.insert("tools-old-a".into());
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Working;
                    session.state.saved.items = vec![
                        json!({"id":"old-user","type":"userMessage"}),
                        json!({"id":"old-a","type":"commandExecution","command":"pwd","status":"completed"}),
                        json!({"id":"old-b","type":"commandExecution","command":"ls","status":"completed"}),
                        json!({"id":"new-user","type":"userMessage"}),
                        json!({"id":"new-a","type":"commandExecution","command":"git fetch","status":"completed"}),
                        json!({"id":"new-b","type":"commandExecution","command":"git status","status":"inProgress"}),
                    ];
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-toggle-tool-1").is_none());
        assert!(cx.debug_bounds("codex-toggle-tool-4").is_none());
        assert!(cx.debug_bounds("codex-toggle-tool-5").is_some());
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.expanded.insert("work-3".into());
                view.session.update(cx, |session, cx| {
                    session.state.saved.items[5]["status"] = json!("completed");
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-toggle-tool-4").is_none());
        assert!(cx.debug_bounds("codex-toggle-tool-5").is_some());
        cx.update(|_, cx| {
            view.update(cx, |view, _| {
                view.expanded.remove("work-3");
            });
        });
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Done;
                    session.state.saved.items[5]["status"] = json!("completed");
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-toggle-tools-1").is_some());
        assert!(cx.debug_bounds("codex-toggle-tools-4").is_none());
        assert!(cx.debug_bounds("codex-toggle-tool-1").is_none());
        assert!(cx.debug_bounds("codex-toggle-tool-4").is_none());
        assert!(cx.debug_bounds("codex-toggle-tool-5").is_none());
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.expanded.insert("work-3".into());
                cx.notify();
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-toggle-tools-4").is_some());
        assert!(cx.debug_bounds("codex-toggle-tool-4").is_some());
        assert!(cx.debug_bounds("codex-toggle-tool-5").is_some());
    }

    #[gpui::test]
    fn consecutive_tools_share_one_expandable_summary(cx: &mut gpui::TestAppContext) {
        let (view, cx, _, _) = fixture(cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.items = vec![
                        json!({"id":"a","type":"commandExecution","command":"pwd"}),
                        json!({"id":"b","type":"mcpToolCall","server":"docs","tool":"search"}),
                        json!({"id":"c","type":"agentMessage","text":"Results"}),
                        json!({"id":"d","type":"commandExecution","command":"ls"}),
                        json!({"id":"e","type":"reasoning","text":"Reviewing"}),
                    ];
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-tool-group-0").is_none());
        assert!(cx.debug_bounds("codex-message-body-2").is_none());
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.expanded.insert("work-0".into());
                cx.notify();
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-tool-group-0").is_some());
        assert!(cx.debug_bounds("codex-message-body-2").is_some());
        assert!(cx.debug_bounds("codex-tool-group-1").is_none());
        assert!(cx.debug_bounds("codex-tool-group-3").is_some());
        assert!(cx.debug_bounds("codex-toggle-tool-0").is_some());
        assert!(cx.debug_bounds("codex-toggle-tool-1").is_some());
        assert!(cx.debug_bounds("codex-toggle-tool-3").is_some());
        let commands = cx.debug_bounds("codex-tool-group-3").unwrap();
        let thought = cx.debug_bounds("codex-tool-group-4").unwrap();
        let compact_gap = cx.update(|window, _| rpx(SPACE_LG).to_pixels(window.rem_size()));
        assert_eq!(thought.top() - commands.bottom(), compact_gap);
        let first = cx.debug_bounds("codex-toggle-tool-0").unwrap();
        let second = cx.debug_bounds("codex-toggle-tool-1").unwrap();
        let row_gap = cx.update(|window, _| rpx(SPACE_XS).to_pixels(window.rem_size()));
        assert_eq!(second.top() - first.bottom(), row_gap);
        assert!(cx.debug_bounds("codex-tool-detail-0").is_none());
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.expanded.insert("tools-a".into());
                cx.notify();
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-toggle-tool-0").is_none());
        assert!(cx.debug_bounds("codex-toggle-tool-1").is_none());
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.expanded.remove("tools-a");
                cx.notify();
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-toggle-tool-0").is_some());
        assert!(cx.debug_bounds("codex-toggle-tool-1").is_some());
        assert!(cx.debug_bounds("codex-tool-detail-0").is_none());
        assert!(cx.debug_bounds("codex-tool-detail-1").is_none());
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.expanded.insert("tool-a".into());
                cx.notify();
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-tool-detail-0").is_some());
        assert!(cx.debug_bounds("codex-tool-detail-1").is_none());
    }

    #[gpui::test]
    fn busy_shortcuts_queue_and_steer_preserve_acknowledged_draft(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Working;
                    session.state.turn_id = Some("active-turn".into());
                    cx.notify();
                });
                view.input
                    .update(cx, |input, cx| input.set_value("follow up", window, cx));
                view.input.focus_handle(cx).focus(window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("enter");
        draw(cx);
        let queue_id = view.read_with(cx, |view, _| view.submitted_ack.unwrap());
        let queued_actions = actions.try_iter().collect::<Vec<_>>();
        assert!(
            queued_actions.iter().any(|action| matches!(action,
            Action::SubmitWithMode { id, text, mode: SubmissionMode::Queue, .. }
                if *id == queue_id && text.trim() == "follow up")),
            "{}",
            queued_actions
                .iter()
                .map(|action| match action {
                    Action::SubmitWithMode { id, text, mode, .. } =>
                        format!("mode={mode:?} id={id} text={text:?}"),
                    Action::Submit { id, text, .. } => format!("send id={id} text={text:?}"),
                    _ => "other".to_owned(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        );
        assert_eq!(
            view.read_with(cx, |view, cx| view.input.read(cx).value().trim().to_owned()),
            "follow up"
        );
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.accepted_submission = Some(queue_id);
                    session.state.queued_prompts.push(codex::QueuedPrompt {
                        id: queue_id,
                        text: "follow up".into(),
                        images: Vec::new(),
                    });
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(view.read_with(cx, |view, cx| view.input.read(cx).value().is_empty()));
        assert!(cx.debug_bounds("codex-queued-prompts").is_some());
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("change direction", window, cx);
                });
                view.input.focus_handle(cx).focus(window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("secondary-enter");
        draw(cx);
        let steer_id = view.read_with(cx, |view, _| view.submitted_ack.unwrap());
        assert!(actions.try_iter().any(|action| matches!(action,
            Action::SubmitWithMode { id, mode: SubmissionMode::Steer, .. } if id == steer_id)));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.rejected_submission = Some(steer_id);
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, cx| view.input.read(cx).value().trim().to_owned()),
            "change direction"
        );
    }

    #[gpui::test]
    fn unsupported_steer_keeps_draft_and_enter_queues(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.provider = codex::Provider::Claude;
                    session.state.status = Status::Working;
                    session.state.turn_id = Some("active-turn".into());
                    cx.notify();
                });
                view.input
                    .update(cx, |input, cx| input.set_value("follow up", window, cx));
                view.input.focus_handle(cx).focus(window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("secondary-enter");
        draw(cx);
        assert!(!actions.try_iter().any(|action| matches!(
            action,
            Action::SubmitWithMode { .. } | Action::Submit { .. }
        )));
        assert_eq!(
            view.read_with(cx, |view, cx| view.input.read(cx).value().trim().to_owned()),
            "follow up"
        );
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(actions.try_iter().any(|action| matches!(
            action,
            Action::SubmitWithMode {
                mode: SubmissionMode::Queue,
                ..
            }
        )));
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(!actions.try_iter().any(|action| matches!(
            action,
            Action::SubmitWithMode { .. } | Action::Submit { .. }
        )));
    }

    #[gpui::test]
    fn shell_shortcut_steers_only_from_main_input_and_preserves_zen_elsewhere(
        cx: &mut gpui::TestAppContext,
    ) {
        struct ShellProbe {
            chat: Entity<CodexView>,
            zen_toggles: usize,
            outside: FocusHandle,
        }
        impl Render for ShellProbe {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .child(self.chat.clone())
                    .child(div().track_focus(&self.outside))
                    .on_action(cx.listener(|this, _: &crate::keymap::ToggleZen, _, _| {
                        this.zen_toggles += 1;
                    }))
            }
        }
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.bind_keys(crate::keymap::shell_bindings());
        });
        let (handle, actions) = Handle::channel();
        let (_updates, receive) = mpsc::channel();
        let mut chat = None;
        let (shell, cx) = cx.add_window_view(|window, cx| {
            let session = cx.new(|cx| {
                CodexSession::from_connection(
                    snapshot(),
                    Connection {
                        handle,
                        updates: receive,
                    },
                    cx,
                )
            });
            let view = cx.new(|cx| CodexView::new(session, window, cx));
            chat = Some(view.clone());
            ShellProbe {
                chat: view,
                zen_toggles: 0,
                outside: cx.focus_handle(),
            }
        });
        let view = chat.unwrap();
        let shell_enter = format!("{}enter", crate::keymap::platform_mod_prefix());
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, _| {
                    session.state.status = Status::Working;
                    session.state.turn_id = Some("active-turn".into());
                });
                view.input
                    .update(cx, |input, cx| input.set_value("steer this", window, cx));
                view.input.focus_handle(cx).focus(window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes(&shell_enter);
        draw(cx);
        assert!(actions.try_iter().any(|action| matches!(
            action,
            Action::SubmitWithMode {
                mode: SubmissionMode::Steer,
                ..
            }
        )));
        assert_eq!(shell.read_with(cx, |shell, _| shell.zen_toggles), 0);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.submitted = None;
                view.submitted_ack = None;
                view.input
                    .update(cx, |input, cx| input.set_value("line", window, cx));
                view.input.focus_handle(cx).focus(window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("shift-enter");
        draw(cx);
        assert!(view.read_with(cx, |view, cx| view.input.read(cx).value().contains('\n')));
        assert!(!actions.try_iter().any(|action| matches!(
            action,
            Action::SubmitWithMode { .. } | Action::Submit { .. }
        )));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, _| {
                    session.state.status = Status::Idle;
                    session.state.turn_id = None;
                });
                view.input
                    .update(cx, |input, cx| input.set_value("send this", window, cx));
            });
        });
        draw(cx);
        cx.simulate_keystrokes(&shell_enter);
        draw(cx);
        assert!(actions
            .try_iter()
            .any(|action| matches!(action, Action::Submit { .. })));
        assert_eq!(shell.read_with(cx, |shell, _| shell.zen_toggles), 0);
        cx.update(|window, cx| {
            let focus = view.read(cx).option_focus[0].clone();
            focus.focus(window, cx);
        });
        draw(cx);
        cx.simulate_keystrokes(&shell_enter);
        draw(cx);
        assert_eq!(shell.read_with(cx, |shell, _| shell.zen_toggles), 1);
        cx.update(|window, cx| {
            let focus = shell.read(cx).outside.clone();
            focus.focus(window, cx);
        });
        draw(cx);
        cx.simulate_keystrokes(&shell_enter);
        draw(cx);
        assert_eq!(shell.read_with(cx, |shell, _| shell.zen_toggles), 2);
    }

    #[gpui::test]
    fn pending_primary_retains_stop_command_and_stop_command_is_deduplicated(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, _| {
                    session.state.status = Status::Working;
                    session.state.turn_id = Some("command-turn".into());
                });
                view.input
                    .update(cx, |input, cx| input.set_value("/stop", window, cx));
                view.submitted = Some("awaiting acknowledgement".into());
                view.submit(window, cx);
                assert_eq!(view.input.read(cx).value().as_ref(), "/stop");
            });
        });
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::Interrupt)));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.submitted = None;
                view.submit(window, cx);
                view.input
                    .update(cx, |input, cx| input.set_value("/stop", window, cx));
                view.submit(window, cx);
                assert_eq!(view.input.read(cx).value().as_ref(), "/stop");
            });
        });
        assert_eq!(
            actions
                .try_iter()
                .filter(|action| matches!(action, Action::Interrupt))
                .count(),
            1
        );
    }

    #[gpui::test]
    fn stopping_is_independent_of_pending_draft_and_guards_repeat_actions(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Working;
                    session.state.turn_id = Some("active-turn".into());
                    cx.notify();
                });
                view.input
                    .update(cx, |input, cx| input.set_value("follow up", window, cx));
                view.submit_mode(SubmissionMode::Queue, window, cx);
                view.interrupt(cx);
                view.interrupt(cx);
            });
        });
        let sent = actions.try_iter().collect::<Vec<_>>();
        assert_eq!(
            sent.iter()
                .filter(|action| matches!(action, Action::Interrupt))
                .count(),
            1
        );
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.submitted = None;
                view.submitted_ack = None;
                view.submit_mode(SubmissionMode::Queue, window, cx);
                view.submit_mode(SubmissionMode::Steer, window, cx);
            });
        });
        assert!(!actions.try_iter().any(|action| matches!(
            action,
            Action::SubmitWithMode { .. } | Action::Submit { .. }
        )));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Done;
                    session.state.turn_id = None;
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(view.read_with(cx, |view, _| view.interrupt_requested_for_turn.is_none()));
    }

    #[gpui::test]
    fn shimmer_moves_bright_glyph_band_and_keeps_unicode_boundaries(cx: &mut gpui::TestAppContext) {
        let (_, cx, _, _) = fixture(cx);
        cx.update(|_, _| {
            let label = "5 tool calls · Running…";
            let early = shimmer_highlights(label, 0.25);
            let later = shimmer_highlights(label, 0.75);
            assert_eq!(early.len(), label.chars().count());
            assert!(early
                .iter()
                .all(|(range, _)| label.is_char_boundary(range.start)
                    && label.is_char_boundary(range.end)));
            let brightest = |runs: &[(std::ops::Range<usize>, gpui::HighlightStyle)]| {
                runs.iter()
                    .enumerate()
                    .max_by(|(_, first), (_, second)| {
                        first
                            .1
                            .color
                            .unwrap()
                            .l
                            .total_cmp(&second.1.color.unwrap().l)
                    })
                    .unwrap()
                    .0
            };
            assert!(brightest(&early) < brightest(&later));
            assert_ne!(
                early[0].1.color,
                early[early.len() / 2].1.color,
                "shimmer varies glyph colors rather than pulsing the whole line"
            );
        });
    }

    #[test]
    fn collapsed_preview_marks_hidden_lines_without_changing_draft() {
        assert_eq!(
            collapsed_draft_preview("First line\nSecond line"),
            "First line…"
        );
        assert_eq!(
            collapsed_draft_preview("Long single line"),
            "Long single line"
        );
        assert_eq!(collapsed_draft_preview("\nSecond line"), "…");
        assert_eq!(collapsed_draft_preview("First line\n"), "First line…");
    }

    #[gpui::test]
    fn reduced_motion_composer_preview_restores_preserved_editor(cx: &mut gpui::TestAppContext) {
        let (view, cx, _, _) = fixture(cx);
        cx.update(|window, cx| {
            cx.set_reduce_motion(true);
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("First line\nSecond line", window, cx);
                    input.set_selected_range(4..4, cx);
                });
                view.input.focus_handle(cx).focus(window, cx);
            });
        });
        draw(cx);
        cx.update(|_, cx| view.update(cx, |view, cx| view.set_composer_collapsed(true, cx)));
        draw(cx);
        assert!(cx.debug_bounds("codex-composer-preview").is_some());
        view.read_with(cx, |view, cx| {
            assert!(view.composer_transition_from.is_none());
            assert_eq!(
                view.input.read(cx).value().as_ref(),
                "First line\nSecond line"
            );
            assert_eq!(view.input.read(cx).cursor(), 4);
        });
        // The original Input remains mounted and owns typed keys under the preview.
        cx.simulate_input("x");
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert!(!view.composer_collapsed);
            assert_eq!(
                view.input.read(cx).value().as_ref(),
                "Firsxt line\nSecond line"
            );
            assert_eq!(view.input.read(cx).cursor(), 5);
        });
        assert!(cx.debug_bounds("codex-composer-preview").is_none());
    }

    #[gpui::test]
    fn transcript_scroll_collapses_composer_and_restores_without_losing_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        let draft = "First line\nSecond line\nThird line\nFourth line";
        let image = format!(
            "data:image/png;base64,{}",
            STANDARD.encode(test_image(image::ImageFormat::Png))
        );
        cx.update(|window, cx| view.update(cx, |view, cx| {
            view.images.push(image.clone());
            view.previews.insert(image.clone(), image_from_url(&image).unwrap());
            view.input.update(cx, |input, cx| input.set_value(draft, window, cx));
            view.input.focus_handle(cx).focus(window, cx);
            view.session.update(cx, |session, cx| {
                session.state.saved.items = vec![json!({"id":"long","type":"agentMessage","text":"Earlier response.\n\n".repeat(80)})];
                cx.notify();
            });
            cx.notify();
        }));
        draw(cx);
        draw(cx);
        let expanded = cx.debug_bounds("codex-composer-input").unwrap().size.height;
        let scroll = |delta, cx: &mut gpui::VisualTestContext| {
            let bounds = cx.debug_bounds("codex-messages").unwrap();
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: bounds.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(delta))),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            });
            draw(cx);
            draw(cx);
        };
        scroll(400.0, cx);
        view.read_with(cx, |view, cx| {
            assert!(
                view.composer_collapsed,
                "upward transcript scroll should enter browse mode"
            );
            assert_eq!(view.input.read(cx).value().as_ref(), draft);
            assert_eq!(view.images, vec![image.clone()]);
        });
        assert!(cx.debug_bounds("codex-composer-input").unwrap().size.height < expanded);
        assert!(cx.debug_bounds("codex-attachments").is_none());
        // The editor retained focus during scrolling; clicking it still expands it.
        let input = cx.debug_bounds("codex-composer-input").unwrap();
        cx.simulate_mouse_down(
            input.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            input.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        draw(cx);
        view.read_with(cx, |view, _| assert!(!view.composer_collapsed));
        assert!(cx.debug_bounds("codex-attachments").is_some());
        scroll(400.0, cx);
        view.read_with(cx, |view, _| assert!(view.composer_collapsed));
        scroll(-100000.0, cx);
        // Restoring the taller composer moves the transcript through the follow animation.
        for _ in 0..80 {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(16));
            draw(cx);
        }
        view.read_with(cx, |view, cx| {
            assert!(
                !view.composer_collapsed,
                "scrolling back to the latest response restores the composer"
            );
            assert_eq!(view.input.read(cx).value().as_ref(), draft);
            assert_eq!(view.images, vec![image]);
            assert_eq!(
                view.scroll.offset().y,
                -view.scroll.max_offset().y,
                "restoring the composer must keep the latest content visible"
            );
        });
        assert!(!actions.try_iter().any(|action| matches!(
            action,
            Action::Submit { .. } | Action::SubmitWithMode { .. }
        )));
    }

    #[gpui::test]
    fn tiny_upward_scroll_stays_collapsed_after_viewport_reaches_bottom(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        cx.update(|_, cx| {
            cx.set_reduce_motion(true);
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.items = vec![json!({"id":"long","type":"agentMessage","text":"Earlier response.\n\n".repeat(80)})];
                    cx.notify();
                });
            });
        });
        draw(cx);
        draw(cx);
        let scroll = |delta, cx: &mut gpui::VisualTestContext| {
            let bounds = cx.debug_bounds("codex-messages").unwrap();
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: bounds.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(delta))),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            });
            draw(cx);
            draw(cx);
        };
        // Cross the browse threshold by less than the composer's height change.
        scroll(SPACE_20 + 1.0, cx);
        view.read_with(cx, |view, _| {
            assert!(view.composer_collapsed);
            assert_eq!(
                view.scroll.offset().y,
                -view.scroll.max_offset().y,
                "collapsing the composer clamps the transcript to its new bottom"
            );
        });
        for _ in 0..4 {
            scroll(1.0, cx);
            view.read_with(cx, |view, _| {
                assert!(view.composer_collapsed, "continued upward scrolling must not restore the composer at a layout-created bottom");
            });
        }
        scroll(-1.0, cx);
        view.read_with(cx, |view, _| {
            assert!(
                !view.composer_collapsed,
                "downward scrolling near the bottom restores the composer"
            );
            assert_eq!(view.scroll.offset().y, -view.scroll.max_offset().y);
        });
        scroll(-1.0, cx);
        view.read_with(cx, |view, _| assert!(!view.composer_collapsed));
    }

    #[gpui::test]
    fn removing_images_removes_and_renumbers_inline_references(cx: &mut gpui::TestAppContext) {
        let (view, cx, _, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.images = vec!["first".into(), "second".into()];
                view.input.update(cx, |input, cx| {
                    input.set_value("Compare Image 1 with Image 2", window, cx);
                });
                view.restore_image_tokens(window, cx);
                assert_eq!(view.picked_tokens.len(), 2);
                view.remove_image(0, window, cx);
                assert_eq!(
                    view.input.read(cx).value().as_ref(),
                    "Compare  with Image 1"
                );
                assert_eq!(view.picked_tokens.len(), 1);
                view.remove_image(0, window, cx);
                assert_eq!(view.input.read(cx).value().as_ref(), "Compare  with ");
                assert!(view.picked_tokens.is_empty());
                assert!(view.images.is_empty());
            });
        });
    }

    #[gpui::test]
    fn stream_reveal_preserves_history_and_reveals_only_appended_text(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.last_content.clear(); // Model mounting an existing saved transcript.
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Working;
                    session.state.saved.items =
                        vec![json!({"id":"history","type":"agentMessage","text":"Saved history"})];
                    cx.notify();
                });
            });
        });
        draw(cx);
        view.read_with(cx, |view, _| {
            assert_eq!(view.stream_text["history"].1, "Saved history".len());
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Working;
                    session.state.saved.items[0]["text"] =
                        json!("Saved history with a Unicode continuation 界界界界界界界界界界界界");
                    cx.notify();
                });
                let _ = view.render(window, cx);
                let (text, shown) = &view.stream_text["history"];
                assert!(*shown >= "Saved history".len());
                assert!(
                    *shown < text.len(),
                    "status={:?}, reduced={}, shown={shown}",
                    view.session.read(cx).state.status,
                    cx.reduce_motion()
                );
                assert!(text.is_char_boundary(*shown));
            });
        });
        draw(cx);
        for _ in 0..80 {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(16));
            draw(cx);
        }
        view.read_with(cx, |view, _| {
            let (text, shown) = &view.stream_text["history"];
            assert_eq!(*shown, text.len());
        });
    }

    #[gpui::test]
    fn streaming_preserves_manual_scroll_and_follows_only_near_bottom(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        let stream = |lines, cx: &mut gpui::VisualTestContext| {
            cx.update(|_, cx| view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Working;
                    session.state.saved.items = vec![json!({"id":"stream","type":"agentMessage","text":"A streaming paragraph.\n\n".repeat(lines)})];
                    cx.notify();
                });
            }));
            draw(cx);
            for _ in 0..120 {
                cx.executor()
                    .advance_clock(std::time::Duration::from_millis(16));
                draw(cx);
            }
        };
        stream(40, cx);
        view.read_with(cx, |view, _| {
            assert_eq!(view.scroll.offset().y, -view.scroll.max_offset().y);
        });
        // A user reading older content must not be pulled down by streaming or incidental renders.
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.scroll
                    .set_offset(gpui::point(gpui::px(0.), gpui::px(-100.)));
                cx.notify();
            });
        });
        draw(cx);
        view.read_with(cx, |view, _| {
            assert_eq!(view.scroll.offset().y, gpui::px(-100.));
        });
        stream(60, cx);
        view.read_with(cx, |view, _| {
            assert_eq!(view.scroll.offset().y, gpui::px(-100.));
        });
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    let text = session.state.saved.items[0]["text"]
                        .as_str()
                        .unwrap()
                        .replace('A', "B");
                    session.state.saved.items[0]["text"] = json!(text);
                    cx.notify();
                });
            });
        });
        draw(cx);
        view.read_with(cx, |view, _| {
            assert_eq!(view.scroll.offset().y, gpui::px(-100.));
        });
        // Returning to the latest response restores automatic following.
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.scroll.scroll_to_bottom();
                cx.notify();
            });
        });
        draw(cx);
        stream(80, cx);
        view.read_with(cx, |view, _| {
            assert_eq!(view.scroll.offset().y, -view.scroll.max_offset().y);
        });
        // An explicit submission follows even when the user had scrolled upward.
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.scroll
                    .set_offset(gpui::point(gpui::px(0.), gpui::px(-100.)));
                view.follow_after_submit = true;
                cx.notify();
            });
        });
        draw(cx);
        let before = view.read_with(cx, |view, _| view.scroll.offset().y);
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(16));
        draw(cx);
        view.read_with(cx, |view, _| {
            assert!(
                view.scroll.offset().y < before,
                "automatic following must move between frames"
            );
            assert!(
                view.scroll.offset().y > -view.scroll.max_offset().y,
                "automatic following must animate before reaching its endpoint"
            );
        });
        for _ in 0..120 {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(16));
            draw(cx);
        }
        view.read_with(cx, |view, _| {
            assert_eq!(view.scroll.offset().y, -view.scroll.max_offset().y);
        });
    }
    #[gpui::test]
    fn option_popup_is_anchored_without_resizing_composer_and_handles_keyboard_and_outside_click(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        draw(cx);
        let input = cx.debug_bounds("codex-composer-input").unwrap();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.open_menu(ComposerMenu::Access, window, cx);
            });
        });
        draw(cx);
        let popup = cx.debug_bounds("codex-options-popup").unwrap();
        assert_eq!(cx.debug_bounds("codex-composer-input").unwrap(), input);
        let trigger = view.read_with(cx, |view, _| view.option_bounds[2].get());
        assert!(popup.bottom() <= trigger.top());
        cx.simulate_keystrokes("down enter");
        draw(cx);
        assert!(actions.try_iter().any(|action| matches!(action, Action::Configure { access: Some(access), .. } if access == "read-only")));
        assert!(view.read_with(cx, |view, _| view.popup.is_none()));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.open_menu(ComposerMenu::Access, window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(view.read_with(cx, |view, _| view.popup.is_none()));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.open_menu(ComposerMenu::Access, window, cx);
            });
        });
        draw(cx);
        cx.simulate_mouse_down(
            gpui::point(gpui::px(10.), gpui::px(10.)),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            gpui::point(gpui::px(10.), gpui::px(10.)),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        draw(cx);
        assert!(view.read_with(cx, |view, _| view.popup.is_none()));
    }

    #[gpui::test]
    fn async_metadata_renders_native_choices_and_custom_answer_without_replacing_composer(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, updates) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("Unrelated draft", window, cx);
                });
                view.images = vec!["draft-image".into()];
            });
        });
        let mut state = snapshot();
        assert!(state.apply(&json!({"method":"item/completed","params":{"threadId":"thread-ui","turnId":"turn-ui","item":{
            "type":"agentMessage","id":"async-ui","text":"Choose scope: All or Composer.","delivery":"async",
            "questions":[{"title":"Choose scope","options":["All","Composer"]}]
        }}})));
        updates.send(state).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(cx.debug_bounds("agent-questions").is_some());
        let other = cx
            .debug_bounds("question-other-0-0")
            .expect("async choices include custom answer");
        cx.simulate_click(other.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("question-input-0-0").is_some());
        cx.simulate_input("Only focus and scrolling");
        draw(cx);
        let original_key = view.read_with(cx, |view, _| {
            view.question_drafts.keys().next().unwrap().clone()
        });
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.turn_id = None;
                    cx.notify();
                });
            });
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert!(view.question_drafts.contains_key(&original_key));
            assert_eq!(
                view.question_drafts[&original_key].questions[0]
                    .input
                    .read(cx)
                    .value()
                    .as_ref(),
                "Only focus and scrolling"
            );
        });
        let submit = cx.debug_bounds("questions-submit-0").unwrap();
        cx.simulate_click(submit.center(), gpui::Modifiers::default());
        draw(cx);
        let answer = actions
            .try_iter()
            .find_map(|action| match action {
                Action::AnswerQuestions(id, Some(answers)) => Some((id, answers)),
                _ => None,
            })
            .expect("native form sends an answer");
        assert_eq!(answer.0, json!("async-ui"));
        assert_eq!(answer.1["0"], vec!["Only focus and scrolling"]);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session
                        .state
                        .question_answer_acks
                        .push(codex::QuestionAnswerAck {
                            sequence: 1,
                            id: json!("async-ui"),
                            thread_id: session.state.saved.thread_id.clone(),
                            turn_id: None,
                        });
                    session.state.questions.clear();
                    session.state.turn_id = Some("async-answer-turn".into());
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("question-receipt-0").is_some());
        view.read_with(cx, |view, cx| {
            assert_eq!(view.input.read(cx).value().as_ref(), "Unrelated draft");
            assert_eq!(view.images, vec!["draft-image"]);
        });
    }

    #[gpui::test]
    fn extended_number_and_text_questions_render_defaults_validate_and_cancel(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        let mut request = codex::QuestionRequest::from_codex(
            json!("extended"),
            &json!({"questions":[
                {"id":"count","question":"How many slides?"},
                {"id":"title","question":"What title?"}
            ]}),
        )
        .unwrap();
        // Core tests cover the Claude wire decoder; this exercises its typed view contract.
        request.title = Some("Before building".into());
        request.questions[0].kind = codex::QuestionKind::Number(codex::QuestionNumber {
            min: 1.,
            max: 20.,
            step: Some(1.),
            default_value: Some(8.),
            unit: Some("slides".into()),
        });
        request.questions[0].description = Some("For the briefing.".into());
        request.questions[1].placeholder = Some("e.g. Quarterly briefing".into());
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.provider = codex::Provider::Claude;
                    session.state.status = Status::Waiting;
                    session.state.questions.push(request.clone());
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("question-input-0-0").is_none());
        assert!(cx.debug_bounds("question-input-0-1").is_some());
        let key = view.read_with(cx, |view, cx| {
            question_key(&view.session.read(cx).state, &request)
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                assert_eq!(
                    view.question_drafts[&key].questions[0]
                        .input
                        .read(cx)
                        .value()
                        .as_ref(),
                    "8"
                );
                let title = view.question_drafts[&key].questions[1].input.clone();
                title.update(cx, |input, cx| {
                    input.set_value("Quarterly briefing", window, cx);
                });
                let number = view.question_drafts[&key].questions[0].input.clone();
                number.update(cx, |input, cx| input.set_value("21", window, cx));
                view.answer_questions(&key, json!("extended"), false, cx);
            });
        });
        draw(cx);
        assert!(actions
            .try_iter()
            .all(|action| !matches!(action, Action::AnswerQuestions(_, _))));
        assert!(cx.debug_bounds("questions-validation-0").is_some());
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let input = view.question_drafts[&key].questions[0].input.clone();
                input.update(cx, |input, cx| input.set_value("12", window, cx));
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("questions-validation-0").is_none());
        let submit = cx.debug_bounds("questions-submit-0").unwrap();
        cx.simulate_click(submit.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(actions.try_iter().any(|action| matches!(action, Action::AnswerQuestions(_, Some(answers)) if answers["count"] == vec!["12"] && answers["title"] == vec!["Quarterly briefing"])));
        // A provider error permits cancelling the same live request without losing the answers.
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.error = Some("Retry input".into());
                    cx.notify();
                });
            });
        });
        draw(cx);
        let cancel = cx.debug_bounds("questions-cancel-0").unwrap();
        cx.simulate_click(cancel.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(actions
            .try_iter()
            .any(|action| matches!(action, Action::AnswerQuestions(_, None))));
    }

    #[gpui::test]
    fn custom_radio_choice_supports_arrow_navigation_back_to_named_options(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        let request = codex::QuestionRequest::from_codex(json!("arrow"), &json!({"questions":[{"id":"layout","question":"Which layout?","isOther":true,"options":[{"label":"Compact","preview":"A compact preview"},{"label":"Roomy"}]}]})).unwrap();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.questions.push(request.clone());
                    cx.notify();
                });
            });
        });
        draw(cx);
        let key = view.read_with(cx, |view, cx| {
            question_key(&view.session.read(cx).state, &request)
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.question_drafts[&key].questions[0].choice_focus[2].focus(window, cx);
            });
        });
        cx.simulate_keystrokes("left");
        draw(cx);
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.question_drafts[&key].questions[0].selected,
                HashSet::from([1])
            );
            assert!(!view.question_drafts[&key].questions[0].custom);
        });
    }

    #[gpui::test]
    fn question_receipts_require_ack_and_redact_secrets(cx: &mut gpui::TestAppContext) {
        let (view, cx, _actions, _updates) = fixture(cx);
        let request = codex::QuestionRequest::from_codex(
            json!("secret-ack"),
            &json!({"questions":[
                {"id":"secret","question":"Enter access token","isSecret":true}
            ]}),
        )
        .unwrap();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Waiting;
                    session.state.questions.push(request.clone());
                    cx.notify();
                });
            });
        });
        draw(cx);
        let key = view.read_with(cx, |view, cx| {
            question_key(&view.session.read(cx).state, &request)
        });
        let anchor = view.read_with(cx, |view, _| view.question_drafts[&key].anchor);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let input = view.question_drafts[&key].questions[0].input.clone();
                input.update(cx, |input, cx| input.set_value("private-token", window, cx));
                view.toggle_question(&key, 0, window, cx);
                view.toggle_question(&key, 0, window, cx);
                assert_eq!(
                    view.question_drafts[&key].questions[0]
                        .input
                        .read(cx)
                        .value()
                        .as_ref(),
                    "private-token"
                );
                view.answer_questions(&key, request.id.clone(), false, cx);
                let pending = view.question_drafts[&key].pending_receipt.as_ref().unwrap();
                assert_eq!(pending.answers[0].1, "Provided securely");
                assert!(view.question_receipts.is_empty());
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("question-receipt-0").is_none());
        // Transport failure keeps drafts, enables retry, and never creates a receipt.
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.error = Some("Transport failed".into());
                    cx.notify();
                });
            });
        });
        draw(cx);
        view.read_with(cx, |view, _| {
            assert!(!view.question_drafts[&key].submitting);
            assert!(view.question_drafts[&key].pending_receipt.is_none());
            assert!(view.question_receipts.is_empty());
        });
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.answer_questions(&key, request.id.clone(), false, cx);
            });
        });
        // A successful acknowledgement and turn transition may arrive in the same snapshot.
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session
                        .state
                        .question_answer_acks
                        .push(codex::QuestionAnswerAck {
                            sequence: 1,
                            id: request.id.clone(),
                            thread_id: session.state.saved.thread_id.clone(),
                            turn_id: session.state.turn_id.clone(),
                        });
                    session.state.questions.clear();
                    session.state.turn_id = Some("next-turn".into());
                    session.state.saved.items.push(
                        json!({"type":"agentMessage","id":"after-answer","text":"Continuing"}),
                    );
                    cx.notify();
                });
            });
        });
        draw(cx);
        let receipt = cx.debug_bounds("question-receipt-0").unwrap();
        let continuation = cx.debug_bounds("codex-message-row-0").unwrap();
        assert!(
            receipt.bottom() <= continuation.top(),
            "receipt stays before later agent activity"
        );
        view.read_with(cx, |view, _| {
            assert!(view.question_drafts.is_empty());
            assert_eq!(view.question_receipts.len(), 1);
            assert_eq!(view.question_receipts[0].anchor, anchor);
            assert_eq!(view.question_receipts[0].answers[0].1, "Provided securely");
        });
    }

    #[gpui::test]
    fn grouped_questions_preserve_drafts_and_submit_all_answers(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, _) = fixture(cx);
        let request = codex::QuestionRequest::from_codex(json!("ask"), &json!({"questions":[
            {"id":"choice","header":"Layout","question":"Which layout?","isOther":true,"options":[{"label":"Compact"},{"label":"Roomy"}]},
            {"id":"features","header":"Features","question":"Which features?","options":[{"label":"Search"},{"label":"History"}]},
            {"id":"token","header":"Token","question":"Enter token","isSecret":true}
        ]})).unwrap();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Waiting;
                    session.state.turn_id = Some("turn-question".into());
                    let mut request = request.clone();
                    request.questions[1].multiple = true;
                    session.state.questions.push(request);
                    cx.notify();
                });
            });
        });
        draw(cx);
        cx.simulate_resize(gpui::size(gpui::px(600.0), gpui::px(480.0)));
        draw(cx);
        let composer = cx.debug_bounds("codex-composer").unwrap();
        let messages = cx.debug_bounds("codex-messages").unwrap();
        let questions = cx.debug_bounds("agent-questions").unwrap();
        assert!(
            composer.bottom() <= gpui::px(480.0),
            "composer footer must remain inside a short viewport: {composer:?}"
        );
        assert!(
            messages.size.height > gpui::px(SPACE_20),
            "questions must leave visible transcript space"
        );
        assert!(
            cx.debug_bounds("agent-controls").is_none(),
            "questions belong in the transcript"
        );
        assert!(questions.size.width <= messages.size.width);
        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        draw(cx);
        for selector in ["questions-cancel-0", "questions-submit-0"] {
            assert_eq!(
                cx.debug_bounds(selector).unwrap().size.height,
                gpui::px(CONTROL_H),
                "question footer uses compact controls"
            );
        }
        let key = view.read_with(cx, |view, cx| {
            question_key(&view.session.read(cx).state, &request)
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.question_drafts[&key].questions[0].choice_focus[0].focus(window, cx);
            });
        });
        cx.simulate_keystrokes("space right");
        draw(cx);
        assert!(
            view.read_with(cx, |view, _| view.question_drafts[&key].questions[0]
                .selected
                .contains(&1))
        );
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.toggle_question(&key, 1, window, cx);
                view.question_drafts[&key].questions[1].choice_focus[0].focus(window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("space");
        draw(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.question_drafts[&key].questions[1].choice_focus[1].focus(window, cx);
            });
        });
        cx.simulate_keystrokes("space");
        draw(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let input = view.question_drafts[&key].questions[2].input.clone();
                input.update(cx, |input, cx| input.set_value(" secret ", window, cx));
                // A streamed item triggers another render while the request stays live.
                view.session.update(cx, |session, cx| {
                    session
                        .state
                        .saved
                        .items
                        .push(json!({"type":"reasoning","id":"stream","content":["working"]}));
                    cx.notify();
                });
            });
        });
        draw(cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.answer_questions(&key, json!("ask"), false, cx);
                view.answer_questions(&key, json!("ask"), false, cx);
            });
        });
        let sent: Vec<_> = actions
            .try_iter()
            .filter_map(|action| {
                if let Action::AnswerQuestions(id, Some(answers)) = action {
                    Some((id, answers))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            sent.len(),
            1,
            "submit is guarded while awaiting provider acknowledgement"
        );
        assert_eq!(sent[0].0, json!("ask"));
        assert_eq!(sent[0].1["choice"], vec!["Roomy"]);
        assert_eq!(sent[0].1["features"], vec!["Search", "History"]);
        assert_eq!(sent[0].1["token"], vec![" secret "]);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.questions.clear();
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(view.read_with(cx, |view, _| view.question_drafts.is_empty()));
        assert!(
            view.read_with(cx, |view, _| view.question_receipts.is_empty()),
            "disappearance without acknowledgement is not successful submission"
        );
        assert!(cx.debug_bounds("agent-questions").is_none());
    }

    #[gpui::test]
    fn mixed_agent_controls_share_scroll_budget_and_emit_typed_responses(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        let request = ElicitationRequest::parse_codex(
            json!("form"),
            &json!({
                "serverName":"Projects", "message":"Configure project", "requestedSchema": {
                    "type":"object", "required":["name","count","enabled","mode","tags"],
                    "properties": {
                        "name":{"type":"string","minLength":2},
                        "count":{"type":"integer","minimum":1},
                        "enabled":{"type":"boolean"},
                        "mode":{"type":"string","enum":["boolean","large"]},
                        "tags":{"type":"array","items":{"type":"string","enum":["one","two"]}},
                        "note":{"type":"string"}
                    }
                }
            }),
        )
        .unwrap();
        let approval = json!({"id":"approval","method":"item/commandExecution/requestApproval","params":{"command":"pwd","cwd":"/tmp"}});
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Waiting;
                    session.state.approvals.push(approval.clone());
                    session.state.elicitations.push(request.clone());
                    session.state.questions.push(
                        codex::QuestionRequest::from_codex(
                            json!("ask-mixed"),
                            &json!({"questions":[{"id":"question","question":"Anything else?"}]}),
                        )
                        .unwrap(),
                    );
                    cx.notify();
                });
            });
        });
        draw(cx);
        cx.simulate_resize(gpui::size(gpui::px(600.0), gpui::px(480.0)));
        draw(cx);
        assert!(cx.debug_bounds("codex-composer").unwrap().bottom() <= gpui::px(480.0));
        assert!(cx.debug_bounds("codex-messages").unwrap().size.height > gpui::px(SPACE_20));
        assert!(
            cx.debug_bounds("agent-controls").unwrap().size.height < gpui::px(QUESTION_PANEL_MAX_H)
        );
        let key = CodexView::elicitation_key(&request);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                assert!(view.elicitation_content(&key, &request, cx).is_err());
                let ElicitationMode::Form(form) = &request.mode else {
                    unreachable!()
                };
                for (index, field) in form.fields.iter().enumerate() {
                    match field.name.as_str() {
                        "name" | "count" => {
                            let input = view.elicitation_drafts[&key].fields[index].input.clone();
                            input.update(cx, |input, cx| {
                                input.set_value(
                                    if field.name == "name" { "Grove" } else { "3" },
                                    window,
                                    cx,
                                );
                            });
                        }
                        "mode" => view.change_elicitation(
                            &key,
                            index,
                            Some("boolean".into()),
                            false,
                            false,
                            cx,
                        ),
                        "tags" => {
                            view.change_elicitation(
                                &key,
                                index,
                                Some("one".into()),
                                true,
                                false,
                                cx,
                            );
                            view.change_elicitation(
                                &key,
                                index,
                                Some("two".into()),
                                true,
                                false,
                                cx,
                            );
                        }
                        "enabled" => view.change_elicitation(&key, index, None, true, false, cx),
                        _ => {}
                    }
                }
                assert!(view.elicitation_content(&key, &request, cx).is_ok());
                view.respond_elicitation(&key, json!("form"), "accept", cx);
                view.respond_elicitation(&key, json!("form"), "accept", cx);
                view.respond_approval(
                    &CodexView::approval_key(&approval),
                    json!("approval"),
                    "allow_once".into(),
                    cx,
                );
            });
        });
        let sent: Vec<_> = actions.try_iter().collect();
        let forms: Vec<_> = sent
            .iter()
            .filter_map(|action| match action {
                Action::RespondElicitation {
                    id,
                    action,
                    content,
                } => Some((id, action, content)),
                _ => None,
            })
            .collect();
        assert_eq!(forms.len(), 1);
        assert_eq!(
            forms[0].2.as_ref().unwrap(),
            &json!({"name":"Grove","count":3,"enabled":true,"mode":"boolean","tags":["one","two"]})
        );
        assert!(sent.iter().any(|action| matches!(action, Action::RespondApproval { choice, .. } if choice == "allow_once")));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.approvals.clear();
                    session.state.elicitations.clear();
                    session.state.questions.clear();
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(
            view.read_with(cx, |view, _| view.elicitation_drafts.is_empty()
                && view.approval_drafts.is_empty())
        );
    }

    #[gpui::test]
    fn approval_edits_are_explicit_and_url_completion_is_separate(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, _) = fixture(cx);
        let approval = json!({"id":"claude-approval","method":"claude/tool","claudeInput":{"command":"pwd"},"claudeRequest":{"tool_name":"Bash","input":{"command":"pwd"}}});
        let request = ElicitationRequest::parse_codex(json!("browser"), &json!({"mode":"url","serverName":"Account","message":"Sign in","url":"https://example.com/sign-in","elicitationId":"sign-in"})).unwrap();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.approvals.push(approval.clone());
                    session.state.elicitations.push(request.clone());
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(
            actions.try_iter().next().is_none(),
            "rendering must not accept an approval or complete a browser request"
        );
        let key = CodexView::approval_key(&approval);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.toggle_approval_edit(&key, cx);
                let input = view.approval_drafts[&key].input.clone();
                input.update(cx, |input, cx| input.set_value("invalid", window, cx));
                view.respond_approval(&key, json!("claude-approval"), "allow_once".into(), cx);
                assert!(!view.approval_drafts[&key].submitting);
                input.update(cx, |input, cx| {
                    input.set_value("{\"command\":\"ls\"}", window, cx);
                });
                view.respond_approval(&key, json!("claude-approval"), "allow_once".into(), cx);
                view.respond_elicitation(
                    &CodexView::elicitation_key(&request),
                    json!("browser"),
                    "decline",
                    cx,
                );
            });
        });
        let sent: Vec<_> = actions.try_iter().collect();
        assert!(sent.iter().any(|action| matches!(action, Action::RespondApproval { input: Some(input), .. } if input == &json!({"command":"ls"}))));
        assert!(sent.iter().any(|action| matches!(action, Action::RespondElicitation { action, content: None, .. } if action == "decline")));
    }

    #[gpui::test]
    fn standalone_mcp_draft_survives_turn_updates_and_expires_with_request(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        let request = ElicitationRequest::parse_codex(json!("standalone"), &json!({"message":"Name", "requestedSchema":{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}})).unwrap();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.elicitations.push(request.clone());
                    cx.notify();
                });
            });
        });
        draw(cx);
        let key = CodexView::elicitation_key(&request);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let input = view.elicitation_drafts[&key].fields[0].input.clone();
                input.update(cx, |input, cx| {
                    input.set_value("Keep this draft", window, cx);
                });
                view.session.update(cx, |session, cx| {
                    session.state.turn_id = Some("unrelated-turn".into());
                    cx.notify();
                });
            });
        });
        draw(cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.turn_id = None;
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, cx| view
                .elicitation_content(&key, &request, cx)
                .unwrap()),
            json!({"name":"Keep this draft"})
        );
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.elicitations.clear();
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(view.read_with(cx, |view, _| view.elicitation_drafts.is_empty()));
    }

    #[gpui::test]
    fn composer_grows_to_limit_and_shrinks_after_clearing(cx: &mut gpui::TestAppContext) {
        let (view, cx, _, _) = fixture(cx);
        draw(cx);
        let height = |cx: &mut gpui::VisualTestContext| {
            cx.debug_bounds("codex-composer-input").unwrap().size.height
        };
        let initial = height(cx);
        let set = |text: String, cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.input
                        .update(cx, |input, cx| input.set_value(text, window, cx));
                });
            });
            draw(cx);
        };
        set("line\n".repeat(6), cx);
        let grown = height(cx);
        assert!(
            grown > initial,
            "typing multiline content must grow the visible composer"
        );
        set("line\n".repeat(14), cx);
        let capped = height(cx);
        assert!(capped > grown);
        set("line\n".repeat(30), cx);
        assert_eq!(height(cx), capped, "long drafts must stop growing");
        set(String::new(), cx);
        assert_eq!(
            height(cx),
            initial,
            "clearing restores compact composer height"
        );
    }

    #[test]
    fn work_headers_use_minutes_and_skip_unknown_history_duration() {
        use serde_json::json;
        let mut state = snapshot();
        state.saved.items = vec![
            json!({"type":"userMessage","id":"prompt"}),
            json!({"type":"reasoning","id":"thought"}),
            json!({"type":"agentMessage","id":"answer","text":"Done"}),
        ];
        assert!(work_header_at(&state, 0).is_none());
        assert_eq!(work_header_at(&state, 1).unwrap().1, "Worked");
        assert!(work_header_at(&state, 2).is_none());
        state.saved.work_periods.push(codex::WorkPeriod {
            start_item: 0,
            end_item: Some(3),
            duration_ms: Some(23_900),
        });
        for (duration_ms, expected) in [
            (0, "Worked for <1m"),
            (23_900, "Worked for <1m"),
            (59_999, "Worked for <1m"),
            (60_000, "Worked for 1m"),
            (125_000, "Worked for 2m"),
        ] {
            state.saved.work_periods[0].duration_ms = Some(duration_ms);
            assert_eq!(work_header_at(&state, 1).unwrap().1, expected);
        }
        assert_eq!(work_key_at(&state, 1), work_key_at(&state, 2));
        state.saved.items.truncate(1);
        state.saved.work_periods.clear();
        state.status = Status::Working;
        assert_eq!(work_header_at(&state, 1).unwrap().1, "Working");
        state.work_started = Some(std::time::Instant::now());
        assert_eq!(work_header_at(&state, 1).unwrap().1, "Working for <1m");
        state.work_started = Some(
            std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(125))
                .unwrap(),
        );
        assert_eq!(work_header_at(&state, 1).unwrap().1, "Working for 2m");
    }

    #[test]
    fn running_turn_shows_only_current_tool_then_reveals_history() {
        let mut state = snapshot();
        state.saved.items = vec![
            serde_json::json!({"type":"userMessage"}),
            serde_json::json!({"type":"commandExecution","status":"completed"}),
            serde_json::json!({"type":"commandExecution","status":"inProgress"}),
        ];
        state.status = Status::Working;
        assert!(!tool_visible(&state, 1));
        assert!(tool_visible(&state, 2));
        state.saved.items[2]["status"] = serde_json::json!("completed");
        assert!(tool_visible(&state, 2));
        state.status = Status::Done;
        assert!(tool_visible(&state, 1));
        assert!(tool_visible(&state, 2));
    }

    #[test]
    fn command_activity_rows_use_tense_without_output() {
        let item = serde_json::json!({"type":"commandExecution","command":"git status\n--short","aggregatedOutput":"private output"});
        assert_eq!(
            tool_activity_summary(&item, true),
            "Running git status --short"
        );
        assert_eq!(
            tool_activity_summary(&item, false),
            "Ran git status --short"
        );
    }

    #[test]
    fn tool_summaries_describe_inputs_without_leaking_results() {
        use serde_json::json;
        for (item, expected) in [
            (
                json!({"type":"commandExecution","command":"cargo test\n--lib","aggregatedOutput":"secret result"}),
                "Command · cargo test --lib",
            ),
            (
                json!({"type":"fileChange","changes":[{"path":"src/main.rs","diff":"secret result"},{"path":"Cargo.toml"}]}),
                "File changes · src/main.rs, Cargo.toml",
            ),
            (
                json!({"type":"mcpToolCall","server":"docs","tool":"search","arguments":{"query":"gpui"},"result":{"content":[{"text":"secret result"}]}}),
                "Tool · docs / search",
            ),
            (
                json!({"type":"webSearch","action":{"query":"GPUI docs"}}),
                "Web search · GPUI docs",
            ),
        ] {
            assert_eq!(tool_summary(&item), expected);
        }
        let detail = tool_detail(
            &json!({"type":"mcpToolCall","server":"docs","tool":"search","arguments":{"query":"gpui"},"result":{"content":[{"text":"returned documentation"}]}}),
        );
        assert!(detail.contains("gpui"));
        assert!(detail.contains("returned documentation"));
    }

    #[gpui::test]
    fn enqueue_does_not_clear_composer_and_provider_rejection_retains_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, updates) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("keep my prompt", window, cx);
                });
                view.submit(window, cx);
            });
        });
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, cx| view.input.read(cx).value().to_string()),
            "keep my prompt"
        );
        assert!(actions
            .try_iter()
            .any(|action| matches!(action,Action::Submit{text,..}if text=="keep my prompt")));
        let mut failed = snapshot();
        failed.status = Status::Failed;
        failed.saved.draft = "keep my prompt".into();
        failed.error = Some("Provider rejected prompt".into());
        failed.rejected_submission = view.read_with(cx, |view, _| view.submitted_ack);
        updates.send(failed).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, cx| view.input.read(cx).value().to_string()),
            "keep my prompt"
        );
    }
    #[gpui::test]
    fn accepted_prompt_clears_composer_once_and_preserves_next_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _actions, updates) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input
                    .update(cx, |input, cx| input.set_value("first", window, cx));
                view.submit(window, cx);
            });
        });
        draw(cx);
        let mut accepted = snapshot();
        accepted.status = Status::Working;
        accepted.accepted_submission = view.read_with(cx, |view, _| view.submitted_ack);
        accepted.turn_id = Some("turn-ui".into());
        updates.send(accepted).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(view.read_with(cx, |view, cx| view.input.read(cx).value().is_empty()));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input
                    .update(cx, |input, cx| input.set_value("next draft", window, cx));
            });
        });
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, cx| view.input.read(cx).value().to_string()),
            "next draft"
        );
    }
    #[gpui::test]
    fn composer_focus_and_secondary_enter_submit_multiline_prompt(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("line one\nline two", window, cx);
                });
                view.focus_handle(cx).focus(window, cx);
            });
        });
        draw(cx);
        assert!(cx.update(|window, cx| view.read(cx).input.focus_handle(cx).is_focused(window)));
        cx.simulate_keystrokes("secondary-enter");
        draw(cx);
        assert!(actions.try_iter().any(
            |action| matches!(action,Action::Submit{text,..}if text.trim()=="line one\nline two")
        ));
    }
    #[gpui::test]
    fn oversized_prompt_is_rejected_before_enqueue_and_keeps_input(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, _) = fixture(cx);
        let text = "x".repeat(512 * 1024 + 1);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input
                    .update(cx, |input, cx| input.set_value(text.clone(), window, cx));
                view.submit(window, cx);
                assert!(view.session.read(cx).state.error.is_some());
                assert_eq!(view.input.read(cx).value().len(), text.len());
            });
        });
        assert!(!actions.try_iter().any(|action| matches!(
            action,
            Action::Prompt(_) | Action::PromptWithImages { .. } | Action::Submit { .. }
        )));
    }
    #[gpui::test]
    fn native_markdown_links_open_artifacts_and_web_urls(cx: &mut gpui::TestAppContext) {
        let (_view, cx, _actions, updates) = fixture(cx);
        for (target, expected) in [
            ("/tmp/option-c.html", "file:///tmp/option-c.html"),
            (
                "/tmp/Option C/设计.html#comparison",
                "file:///tmp/Option%20C/%E8%AE%BE%E8%AE%A1.html#comparison",
            ),
            (
                "/tmp/Option%20C/main.rs#L42",
                "file:///tmp/Option%20C/main.rs#L42",
            ),
            ("https://example.com/a?q=b#c", "https://example.com/a?q=b#c"),
            ("file:///tmp/option-c.html", "file:///tmp/option-c.html"),
            ("mailto:hello@example.com", "mailto:hello@example.com"),
        ] {
            let mut response = snapshot();
            response.saved.items.push(serde_json::json!({
                "type":"agentMessage", "id":"answer",
                "text":format!("[three variations](<{target}>)")
            }));
            updates.send(response).unwrap();
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(40));
            draw(cx);
            let body = cx
                .debug_bounds("codex-message-body-0")
                .expect("rendered Markdown body");
            let position = body.origin + gpui::point(gpui::px(8.), gpui::px(8.));
            cx.simulate_click(position, gpui::Modifiers::default());
            assert_eq!(cx.opened_url().as_deref(), Some(expected), "{target}");
        }
    }

    #[gpui::test]
    fn native_markdown_drag_selects_and_copies_live_response(cx: &mut gpui::TestAppContext) {
        let (_view, cx, _actions, updates) = fixture(cx);
        let mut response = snapshot();
        response.saved.items.push(serde_json::json!({"type":"agentMessage", "id":"answer", "text":"quick selection value\n\n## Details\n\n- formatted list\n\n```rust\n    let selected = true;\n```"}));
        updates.send(response.clone()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        let body = cx
            .debug_bounds("codex-message-body-0")
            .expect("rendered Markdown body");
        let start = body.origin + gpui::point(gpui::px(1.), gpui::px(8.));
        let end = body.origin + gpui::point(gpui::px(110.), gpui::px(8.));
        cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            end,
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(end, gpui::MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        let selected = cx.update(gpui_component::WindowExt::selected_text);
        assert!(
            !selected.trim().is_empty(),
            "drag must highlight actual output text"
        );
        #[cfg(target_os = "macos")]
        cx.simulate_keystrokes("cmd-c");
        #[cfg(not(target_os = "macos"))]
        cx.simulate_keystrokes("ctrl-c");
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|clip| clip.text())),
            Some(selected.clone())
        );
        response.saved.items[0]["text"] = serde_json::json!("quick selection value\n\n## Details\n\n- formatted list\n\n```rust\n    let selected = true;\n```\n\nMore live text.");
        updates.send(response).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(
            cx.debug_bounds("codex-message-body-0").unwrap().size.height > body.size.height,
            "streamed Markdown must actually parse and grow before checking selection"
        );
        assert_eq!(
            cx.update(gpui_component::WindowExt::selected_text),
            selected,
            "streaming must preserve existing selection"
        );
    }

    #[gpui::test]
    fn attachment_squares_clear_the_input_and_tool_text_aligns_with_messages(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.expanded.insert("work-0".into());
                for (width, height) in [(344, 666), (1320, 500)] {
                    let mut bytes = std::io::Cursor::new(Vec::new());
                    image::DynamicImage::new_rgba8(width, height)
                        .write_to(&mut bytes, image::ImageFormat::Png)
                        .unwrap();
                    let url = format!(
                        "data:image/png;base64,{}",
                        STANDARD.encode(bytes.into_inner())
                    );
                    view.previews
                        .insert(url.clone(), image_from_url(&url).unwrap());
                    view.images.push(url);
                }
                view.session.update(cx, |session, cx| {
                    session.state.saved.items = vec![
                        json!({"id":"a","type":"commandExecution","command":"pwd"}),
                        json!({"id":"b","type":"agentMessage","text":"Ready"}),
                    ];
                    cx.notify();
                });
                cx.notify();
            });
        });
        for width in [600.0, 1280.0] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.0)));
            draw(cx);
            let edge = cx.update(|window, _| rpx(IMAGE_PREVIEW_EDGE).to_pixels(window.rem_size()));
            let attachments = cx.debug_bounds("codex-attachments").unwrap();
            let input = cx.debug_bounds("codex-composer-input").unwrap();
            let composer = cx.debug_bounds("codex-composer").unwrap();
            let hints = cx.debug_bounds("codex-composer-hints").unwrap();
            assert!(
                hints.top() >= composer.bottom(),
                "instructions belong outside the border"
            );
            assert!(
                attachments.top() > composer.top() && input.top() > attachments.bottom(),
                "attachments need clear space above and below"
            );
            for selector in ["codex-image-0", "codex-image-1"] {
                let image = cx.debug_bounds(selector).unwrap();
                assert_eq!(image.size.width, edge);
                assert_eq!(image.size.height, edge);
                assert!(image.bottom() <= input.top(), "attachment overlaps input");
                assert!(image.left() >= attachments.left() && image.right() <= attachments.right());
            }
            assert_eq!(attachments.size.height, edge);
            let icon = cx.debug_bounds("codex-tool-icon-0").unwrap();
            let label = cx.debug_bounds("codex-tool-row-label-0").unwrap();
            let body = cx.debug_bounds("codex-message-body-1").unwrap();
            assert_eq!(
                icon.left(),
                body.left(),
                "tool icon must begin at the transcript rail"
            );
            assert!(
                label.left() > icon.right(),
                "tool label follows its icon with a gap"
            );
        }
    }

    #[gpui::test]
    fn user_message_attachments_are_small_squares_below_text(cx: &mut gpui::TestAppContext) {
        let (view, cx, _, _) = fixture(cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                let mut content = vec![json!({"type":"text", "text":"Review these screenshots"})];
                for (width, height) in [(344, 666), (1320, 500)] {
                    let mut bytes = std::io::Cursor::new(Vec::new());
                    image::DynamicImage::new_rgba8(width, height)
                        .write_to(&mut bytes, image::ImageFormat::Png)
                        .unwrap();
                    let url = format!(
                        "data:image/png;base64,{}",
                        STANDARD.encode(bytes.into_inner())
                    );
                    content.push(json!({"type":"image", "url":url}));
                }
                view.session.update(cx, |session, cx| {
                    session.state.saved.items =
                        vec![json!({"id":"screenshots", "type":"userMessage", "content":content})];
                    cx.notify();
                });
                cx.notify();
            });
        });
        for width in [320.0, 1280.0] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.0)));
            draw(cx);
            let edge = cx.update(|window, _| rpx(IMAGE_PREVIEW_EDGE).to_pixels(window.rem_size()));
            let body = cx.debug_bounds("codex-message-body-0").unwrap();
            let attachments = cx.debug_bounds("codex-message-attachments-0").unwrap();
            let label = cx.debug_bounds("codex-message-attachment-count-0").unwrap();
            let row = cx.debug_bounds("codex-message-images-0").unwrap();
            assert!(body.bottom() <= attachments.top());
            assert!(label.bottom() <= row.top());
            for selector in ["codex-message-image-0-0", "codex-message-image-0-1"] {
                let image = cx.debug_bounds(selector).unwrap();
                assert_eq!(image.size.width, edge);
                assert_eq!(image.size.height, edge);
                assert!(image.left() >= row.left() && image.right() <= row.right());
                assert!(image.top() >= row.top() && image.bottom() <= row.bottom());
            }
        }
    }

    #[gpui::test]
    fn composer_pastes_text_and_image_and_submits_image_only(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| {
            view.read(cx).input.focus_handle(cx).focus(window, cx);
            cx.write_to_clipboard(gpui::ClipboardItem::new_string("pasted context".into()));
            window.dispatch_action(Box::new(gpui_component::input::Paste), cx);
        });
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, cx| view.input.read(cx).value().to_string()),
            "pasted context"
        );
        let png = test_image(image::ImageFormat::Png);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input
                    .update(cx, |input, cx| input.set_value("", window, cx));
            });
            cx.write_to_clipboard(gpui::ClipboardItem::new_image(&gpui::Image::from_bytes(
                gpui::ImageFormat::Png,
                png,
            )));
            window.dispatch_action(Box::new(gpui_component::input::Paste), cx);
        });
        draw(cx);
        assert_eq!(view.read_with(cx, |view, _| view.images.len()), 1);
        assert!(cx.debug_bounds("codex-attachments").is_some());
        cx.simulate_keystrokes("enter");
        draw(cx);
        let sent: Vec<_> = actions
            .try_iter()
            .filter_map(|action| {
                if let Action::Submit { text, images, .. } = action {
                    Some((text, images))
                } else {
                    None
                }
            })
            .collect();
        assert!(
            sent.iter().any(|(text, images)| text.trim() == "Image 1"
                && images.len() == 1
                && images[0].starts_with("data:image/png;base64,")),
            "sent={sent:?}; error={:?}; input={:?}; focused={}",
            view.read_with(cx, |view, cx| view.session.read(cx).state.error.clone()),
            view.read_with(cx, |view, cx| view.input.read(cx).value().to_string()),
            cx.update(|window, cx| view.read(cx).input.focus_handle(cx).is_focused(window))
        );
    }

    #[gpui::test]
    fn composer_shift_enter_inserts_newline_and_enter_sends(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input
                    .update(cx, |input, cx| input.set_value("first line", window, cx));
                view.input.focus_handle(cx).focus(window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("shift-enter");
        draw(cx);
        assert!(view.read_with(cx, |view, cx| view.input.read(cx).value().contains('\n')));
        assert!(!actions.try_iter().any(|action| matches!(
            action,
            Action::Prompt(_) | Action::PromptWithImages { .. } | Action::Submit { .. }
        )));
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(actions.try_iter().any(
            |action| matches!(action, Action::Submit{text,..} if text.contains("first line"))
        ));
    }
    #[gpui::test]
    fn root_preserves_raw_terminal_control_c_and_tab(cx: &mut gpui::TestAppContext) {
        struct RawTerminal {
            focus: FocusHandle,
            keys: Vec<String>,
        }
        impl Render for RawTerminal {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .id("terminal-key-probe")
                    .size_full()
                    .track_focus(&self.focus)
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                        this.keys.push(event.keystroke.key.clone());
                        cx.stop_propagation();
                    }))
            }
        }
        cx.update(gpui_component::init);
        let mut terminal = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| RawTerminal {
                focus: cx.focus_handle(),
                keys: Vec::new(),
            });
            let focus = view.read(cx).focus.clone();
            focus.focus(window, cx);
            terminal = Some(view.clone());
            gpui_component::Root::new(view, window, cx).bordered(false)
        });
        draw(cx);
        cx.simulate_keystrokes("ctrl-c tab");
        let keys = terminal.unwrap().read_with(cx, |view, _| view.keys.clone());
        assert_eq!(
            keys,
            ["c", "tab"],
            "Root must not capture raw PTY shortcuts"
        );
    }

    #[gpui::test]
    fn streaming_updates_do_not_duplicate_queued_text(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let state = cx.new(|cx| gpui_component::text::TextViewState::markdown("first", cx));
        state.update(cx, |state, cx| {
            state.set_text_incremental("first second", cx);
            state.set_text_incremental("first second", cx);
            state.set_text_incremental("first second third", cx);
            state.set_text_incremental("first second third", cx);
        });
        cx.run_until_parked();
        assert_eq!(
            state.read_with(cx, |state, _| state.source().to_string()),
            "first second third"
        );
    }

    #[gpui::test]
    fn native_code_copy_preserves_indentation_and_escape_clears_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_view, cx, _actions, updates) = fixture(cx);
        let mut response = snapshot();
        response.saved.items.push(serde_json::json!({"type":"agentMessage", "id":"code", "text":"```rust\n    let selected = true;\n```"}));
        updates.send(response).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        let body = cx.debug_bounds("codex-message-body-0").unwrap();
        let start = body.origin + gpui::point(gpui::px(14.), gpui::px(24.));
        cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(start, gpui::MouseButton::Left, gpui::Modifiers::default());
        #[cfg(target_os = "macos")]
        cx.simulate_keystrokes("cmd-a cmd-c");
        #[cfg(not(target_os = "macos"))]
        cx.simulate_keystrokes("ctrl-a ctrl-c");
        draw(cx);
        let selected = cx.update(gpui_component::WindowExt::selected_text);
        assert!(
            selected.starts_with("    let selected"),
            "code indentation was lost: {selected:?}"
        );
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|clip| clip.text())),
            Some(selected)
        );
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(cx
            .update(gpui_component::WindowExt::selected_text)
            .is_empty());
    }
    #[gpui::test]
    fn image_only_submission_waits_for_explicit_ack_and_retains_rejected_images(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, updates) = fixture(cx);
        let png = test_image(image::ImageFormat::Png);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.paste_images(
                    vec![gpui::Image::from_bytes(gpui::ImageFormat::Png, png)],
                    window,
                    cx,
                );
                let _ = window;
            });
        });
        draw(cx);
        cx.update(|window, cx| view.update(cx, |view, cx| view.submit(window, cx)));
        draw(cx);
        assert_eq!(view.read_with(cx, |view, _| view.images.len()), 1);
        assert!(view.read_with(cx, |view, _| view.submitted.is_some()));
        updates.send(snapshot()).unwrap(); // queued idle snapshot before the provider sees the request
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(
            view.read_with(cx, |view, _| view.submitted.is_some()),
            "stale idle must not count as an ack"
        );
        assert_eq!(view.read_with(cx, |view, _| view.images.len()), 1);
        let image = view.read_with(cx, |view, _| view.images[0].clone());
        let mut rejected = snapshot();
        rejected.error = Some("Provider rejected image".into());
        rejected.rejected_submission = view.read_with(cx, |view, _| view.submitted_ack);
        rejected.saved.draft_images = vec![image.clone()];
        rejected.status = Status::Failed;
        updates.send(rejected.clone()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.images.clone()),
            vec![image]
        );
        assert!(view.read_with(cx, |view, _| view.submitted.is_none()));
        // The immutable clipboard image retains its GPUI image identity across renders.
        let before = view.read_with(cx, |view, _| view.previews.values().next().unwrap().clone());
        draw(cx);
        let after = view.read_with(cx, |view, _| view.previews.values().next().unwrap().clone());
        assert!(Arc::ptr_eq(&before, &after));
        cx.update(|window, cx| view.update(cx, |view, cx| view.submit(window, cx)));
        assert_eq!(
            actions
                .try_iter()
                .filter(|action| matches!(action, Action::Submit { .. }))
                .count(),
            2,
            "rejected image-only draft remains retryable"
        );
        draw(cx);
        let retry_id = view
            .read_with(cx, |view, _| view.submitted_ack)
            .expect("retry stays pending through render");
        updates.send(rejected).unwrap(); // a queued rejection for the previous request
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.submitted_ack),
            Some(retry_id)
        );
        assert_eq!(view.read_with(cx, |view, _| view.images.len()), 1);
        let mut accepted = snapshot();
        accepted.accepted_submission = Some(retry_id);
        accepted.status = Status::Working;
        updates.send(accepted).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(view.read_with(cx, |view, _| view.images.is_empty()));
        assert!(view.read_with(cx, |view, _| view.submitted_ack.is_none()));
    }
    #[gpui::test]
    fn screenshot_tiff_paste_is_prepared_before_prompt_and_normalized_to_png(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        let tiff = test_image(image::ImageFormat::Tiff);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("inspect screenshot", window, cx);
                });
                view.paste_images(
                    vec![gpui::Image::from_bytes(gpui::ImageFormat::Tiff, tiff)],
                    window,
                    cx,
                );
                assert_eq!(view.pending_image_pastes, 1);
                view.submit(window, cx);
            });
        });
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::Submit { .. })));
        draw(cx);
        assert_eq!(view.read_with(cx, |view, _| view.pending_image_pastes), 0);
        assert!(view.read_with(cx, |view, _| view.images[0]
            .starts_with("data:image/png;base64,")));
        cx.update(|window, cx| view.update(cx, |view, cx| view.submit(window, cx)));
        assert!(actions.try_iter().any(|action| matches!(action, Action::Submit {text,images,..} if text.trim()=="Image 1 inspect screenshot" && images.len()==1 && images[0].starts_with("data:image/png;base64,"))));
    }
}
