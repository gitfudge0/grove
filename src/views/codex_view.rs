//! Native conversation surface for a structured Codex session.
use super::status::{self, StatusKind};
use super::{
    components::{
        attached_validation, form_action, header_control, header_control_frame,
        header_control_with_hover, header_control_with_tooltip, preference_choices,
        project_field_well, question_choice, question_disclosure, settings_control, shimmer_label,
        task_mark, task_meter, tooltip, tooltip_content, transcript_action,
        truncation_safe_label_width, TaskPhase,
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
    input::{
        IndentInline, Input, InputEvent, InputState, MoveDown, MoveUp, TextDecoration,
        TextDecorationCollection, TextReplacement,
    },
    menu::{PopupMenu, PopupMenuItem},
    text::{TextSelectionSnapshot, TextView, TextViewState, TextViewStyle},
    Disableable, WindowExt as _,
};
use grove_core::codex::{self, Action, SubmissionMode};
use grove_core::elicitation::{ElicitationKind, ElicitationMode, ElicitationRequest};
use grove_core::tasks::{self, TaskList, TaskStatus};
use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    fmt::Write as _,
    hash::{Hash, Hasher},
    rc::Rc,
    sync::Arc,
};

#[path = "model_picker.rs"]
mod model_picker;

const CODE_COPY_FEEDBACK_MS: u64 = 1200;

/// Paint Markdown source without replacing text or changing editor mode. Leaf
/// spans are ordered and disjoint, so nested emphasis composes correctly with
/// independent picked-file/image decorations and UTF-8 cursor offsets.
fn composer_markdown_decorations(text: &str) -> Vec<TextDecoration> {
    use markdown::mdast::Node;
    fn visit(node: &Node, mut style: gpui::HighlightStyle, out: &mut Vec<TextDecoration>) {
        match node {
            Node::Strong(_) | Node::Heading(_) => {
                style.font_weight = Some(gpui::FontWeight::BOLD);
            }
            Node::Emphasis(_) => style.font_style = Some(gpui::FontStyle::Italic),
            Node::Delete(_) => {
                style.strikethrough = Some(gpui::StrikethroughStyle {
                    thickness: gpui::px(1.),
                    color: None,
                });
            }
            Node::Link(_) | Node::LinkReference(_) => {
                style.underline = Some(gpui::UnderlineStyle {
                    thickness: gpui::px(1.),
                    ..Default::default()
                });
            }
            Node::InlineCode(_) | Node::Code(_) => {
                style.background_color = Some(c::BG_HOVER());
            }
            _ => {}
        }
        if let Some(children) = node.children() {
            for child in children {
                visit(child, style, out);
            }
        } else if style != gpui::HighlightStyle::default() {
            if let Some(position) = node.position() {
                let range = position.start.offset..position.end.offset;
                if !range.is_empty() {
                    out.push(TextDecoration::new(range, style));
                }
            }
        }
    }
    let mut decorations = Vec::new();
    if let Ok(root) = markdown::to_mdast(text, &markdown::ParseOptions::gfm()) {
        visit(&root, gpui::HighlightStyle::default(), &mut decorations);
    }
    decorations
}

/// Keep Markdown in the input model while projecting syntax into native text.
fn composer_markdown_replacements(text: &str) -> Vec<TextReplacement> {
    use markdown::mdast::Node;
    fn hide(range: std::ops::Range<usize>, out: &mut Vec<TextReplacement>) {
        if !range.is_empty() {
            out.push(TextReplacement {
                range,
                text: String::new(),
                font_scale: 1.,
            });
        }
    }
    fn visit(node: &Node, text: &str, out: &mut Vec<TextReplacement>) {
        let Some(position) = node.position() else {
            return;
        };
        let range = position.start.offset..position.end.offset;
        match node {
            Node::Heading(heading) => {
                if let Some(first) = heading.children.first().and_then(Node::position) {
                    let prefix = range.start..first.start.offset;
                    // ATX headings; setext underlines remain literal until supported.
                    if text
                        .get(prefix.clone())
                        .is_some_and(|s| s.trim_start().starts_with('#'))
                    {
                        out.push(TextReplacement {
                            range: prefix,
                            text: String::new(),
                            font_scale: match heading.depth {
                                1 => 1.25,
                                2 => 1.125,
                                _ => 1.,
                            },
                        });
                        if let Some(last) = heading.children.last().and_then(Node::position) {
                            hide(last.end.offset..range.end, out);
                        }
                    }
                }
            }
            Node::ListItem(_) => {
                let line = text[range.clone()].lines().next().unwrap_or("");
                let marker_end = line.find([' ', '\t']).unwrap_or(0);
                let marker = &line[..marker_end];
                if matches!(marker, "-" | "*" | "+") {
                    out.push(TextReplacement {
                        range: range.start..range.start + marker_end,
                        text: "•".into(),
                        font_scale: 1.,
                    });
                }
            }
            Node::Strong(_)
            | Node::Emphasis(_)
            | Node::Delete(_)
            | Node::Link(_)
            | Node::LinkReference(_) => {
                if let Some(children) = node.children() {
                    if let (Some(first), Some(last)) = (
                        children.first().and_then(Node::position),
                        children.last().and_then(Node::position),
                    ) {
                        hide(range.start..first.start.offset, out);
                        hide(last.end.offset..range.end, out);
                    }
                }
            }
            Node::InlineCode(_) => {
                let source = &text[range.clone()];
                let ticks = source.bytes().take_while(|b| *b == b'`').count();
                if ticks > 0 && source.len() >= ticks * 2 {
                    hide(range.start..range.start + ticks, out);
                    hide(range.end - ticks..range.end, out);
                }
            }
            _ => {}
        }
        if let Some(children) = node.children() {
            for child in children {
                visit(child, text, out);
            }
        }
    }
    let mut replacements = Vec::new();
    if let Ok(root) = markdown::to_mdast(text, &markdown::ParseOptions::gfm()) {
        visit(&root, text, &mut replacements);
    }
    replacements.sort_by_key(|r| (r.range.start, r.range.end));
    replacements
}

/// Yolo belongs to the saved native launch recipe, never to a later access choice.
fn session_launched_with_yolo(state: &codex::Snapshot) -> bool {
    if state.saved.native_config_version == 0 {
        // Legacy CLI-only bypass preferences are removed by reconnect migration.
        return false;
    }
    let args = &state.saved.config_args;
    match state.saved.provider {
        codex::Provider::Claude => args
            .iter()
            .any(|arg| arg == "--dangerously-skip-permissions"),
        codex::Provider::Codex => {
            let configured = |value: &str| {
                args.windows(2)
                    .any(|pair| matches!(pair[0].as_str(), "-c" | "--config") && pair[1] == value)
            };
            configured("approval_policy=\"never\"")
                && configured("sandbox_mode=\"danger-full-access\"")
        }
    }
}

fn session_has_yolo_access(state: &codex::Snapshot) -> bool {
    if !session_launched_with_yolo(state) {
        return false;
    }
    let access = state.access.as_deref().or(state.resolved_access.as_deref());
    match state.saved.provider {
        codex::Provider::Claude => access == Some("bypassPermissions"),
        codex::Provider::Codex => {
            state.access.as_deref() == Some("full-access")
                || (state.access.is_none()
                    && state
                        .sandbox
                        .as_ref()
                        .and_then(|value| value["type"].as_str())
                        == Some("dangerFullAccess")
                    && state
                        .approval_policy
                        .as_ref()
                        .and_then(serde_json::Value::as_str)
                        == Some("never"))
        }
    }
}

/// Fixed-size content comparison without serializing or retaining the transcript.
/// Hash full values, including equal-length streaming edits and optimistic items.
fn transcript_content_signature(items: &[serde_json::Value], thread: Option<&str>) -> [u64; 2] {
    std::array::from_fn(|domain| {
        let mut hasher = rustc_hash::FxHasher::default();
        domain.hash(&mut hasher);
        thread.hash(&mut hasher);
        items.len().hash(&mut hasher);
        for item in items {
            hash_transcript_value(item, &mut hasher);
        }
        hasher.finish()
    })
}

/// Value's Hash implementation canonicalizes objects using an allocated sorted
/// vector when serde_json preserves insertion order. Render comparison only needs
/// the same ordered structure as JSON serialization, so visit entries directly.
fn hash_transcript_value(value: &serde_json::Value, hasher: &mut impl Hasher) {
    std::mem::discriminant(value).hash(hasher);
    match value {
        serde_json::Value::Null => {}
        serde_json::Value::Bool(value) => value.hash(hasher),
        serde_json::Value::Number(value) => value.hash(hasher),
        serde_json::Value::String(value) => value.hash(hasher),
        serde_json::Value::Array(values) => {
            values.len().hash(hasher);
            for value in values {
                hash_transcript_value(value, hasher);
            }
        }
        serde_json::Value::Object(values) => {
            values.len().hash(hasher);
            for (key, value) in values {
                key.hash(hasher);
                hash_transcript_value(value, hasher);
            }
        }
    }
}

/// Immutable preparation shared by scroll/animation frames. Session notifications
/// replace the source snapshot; local mutations fork it synchronously.
/// Optimistic bubbles are a separate cache input.
struct TranscriptRenderCache {
    source: Rc<codex::Snapshot>,
    optimistic: Vec<serde_json::Value>,
    state: Rc<codex::Snapshot>,
    content: [u64; 2],
    task_key: u64,
    stream_keys: HashSet<String>,
    has_timestamp: bool,
    response_groups: Vec<ResponseGroup>,
    response_ends: Vec<Option<usize>>,
    response_prose: Vec<Option<usize>>,
    minimap_entries: Vec<MessageMinimapEntry>,
}

impl TranscriptRenderCache {
    fn matches(&self, source: &Rc<codex::Snapshot>, optimistic: &[serde_json::Value]) -> bool {
        Rc::ptr_eq(&self.source, source) && self.optimistic == optimistic
    }

    fn new(source: Rc<codex::Snapshot>, optimistic: Vec<serde_json::Value>) -> Self {
        let mut state = source.clone();
        if !optimistic.is_empty() {
            Rc::make_mut(&mut state)
                .saved
                .items
                .extend(optimistic.iter().cloned());
        }
        let items = &state.saved.items;
        let content = transcript_content_signature(items, state.saved.thread_id.as_deref());
        let task_key = task_cache_key(items);
        let stream_keys = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                item["id"]
                    .as_str()
                    .map_or_else(|| format!("message-{index}"), str::to_owned)
            })
            .collect();
        let has_timestamp = items.iter().any(|item| item["groveReceivedAt"].is_u64());
        let response_groups = response_groups(items, state.saved.thread_id.as_deref());
        let mut response_ends = vec![None; items.len() + 1];
        let mut response_prose = vec![None; items.len()];
        for (group_index, group) in response_groups.iter().enumerate() {
            response_ends[group.end] = Some(group_index);
            for index in &group.prose {
                response_prose[*index] = Some(group_index);
            }
        }
        let minimap_entries = message_minimap_entries(items);
        Self {
            source,
            optimistic,
            state,
            content,
            task_key,
            stream_keys,
            has_timestamp,
            response_groups,
            response_ends,
            response_prose,
            minimap_entries,
        }
    }
}

const STOP_HOVER_OPACITY: f32 = 0.8;
pub(super) const CHAT_MAX_W: f32 = 760.0;
const COMPOSER_MENU_W: f32 = 320.0;
/// Compact reference card, kept local rather than adding a one-use scale tier.
const MODEL_EFFORT_CARD_W: f32 = 256.0;
const MINIMAP_PREVIEW_W: f32 = 320.0;
const COMPOSER_MENU_MAX_H: f32 = 280.0;
const COMPOSER_MIN_H: f32 = 72.0;
const DASHBOARD_COMPOSER_MIN_H: f32 = 48.0;
const USER_BUBBLE_MAX_W: f32 = 600.0;
const IMAGE_PREVIEW_EDGE: f32 = 56.0;
const COMPOSER_TOKEN_UNDERLINE: f32 = 1.0;
const QUEUED_PROMPTS_MAX_H: f32 = 144.0;
const QUESTION_PANEL_MAX_H: f32 = 360.0;
/// Leave room for the transcript and composer even when permission forms are long.
const APPROVAL_TRAY_VIEWPORT_SHARE: f32 = 0.45;
const INLINE_REFERENCE_MAX_H: f32 = 80.0;
const REFERENCE_PREVIEW_H: f32 = 60.0;
const REFERENCE_EXPANDED_H: f32 = 200.0;
const REFERENCE_PREVIEW_CHARS: usize = 240;
const TOOL_DETAIL_MAX_H: f32 = 280.0;
/// Context share, in whole percent, at which the meter switches to the warning colour.
const CONTEXT_WARN_PERCENT: u32 = 80;
/// The context ring's stroke, the icon catalog's optical stroke.
const CONTEXT_RING_STROKE: f32 = 1.5;
/// Polyline segments per full turn of the context ring.
const CONTEXT_RING_SEGMENTS: f32 = 48.0;
/// The task strip header uses the compact control height.
const TASK_STRIP_HEAD_H: f32 = CONTROL_H;
/// One task row in the expanded strip (DESIGN.md row-h).
const TASK_ROW_H: f32 = CONTROL_H + SPACE_SM;
/// One full turn of the running task mark.
const TASK_SPINNER_PERIOD: std::time::Duration =
    std::time::Duration::from_millis(MOTION_SLOW_MS * 4);
/// Clock ticks per spinner frame, the inverse of `animation_clock::spinner_frame`.
const SPINNER_TICKS_PER_FRAME: u64 = 3;

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

fn relative_message_time(timestamp: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let seconds = now.saturating_sub(timestamp) / 1000;
    match seconds {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", seconds / 60),
        3600..86400 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86400),
    }
}

fn action_key(event: &gpui::KeyDownEvent) -> bool {
    matches!(event.keystroke.key.as_str(), "enter" | "space") && !event.is_held
}

/// Icon-only control that copies `text` verbatim, by pointer or keyboard.
fn copy_control(id: &'static str, label: &'static str, text: String) -> gpui::Stateful<gpui::Div> {
    let keyboard = text.clone();
    header_control(id, label)
        .debug_selector(move || id.into())
        .tab_index(0)
        .focus_visible(|style| style.bg(c::BG_HOVER()))
        .child(crate::icons::icon("copy", ICON_MD, c::FG_DIM()))
        .on_key_down(move |event, _, cx| {
            if action_key(event) {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(keyboard.clone()));
                cx.stop_propagation();
            }
        })
        .on_click(move |_, _, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
        })
}

fn composer_icon(
    id: &'static str,
    label: &'static str,
    icon: &'static str,
    disabled: bool,
    primary: bool,
) -> gpui::Stateful<gpui::Div> {
    let fill = if icon == "stop" { c::RED() } else { c::FG() };
    header_control_with_hover(
        id,
        label,
        if disabled {
            if primary {
                fill
            } else {
                c::FIELD_FILL()
            }
        } else if primary {
            if icon == "stop" {
                c::RED().opacity(STOP_HOVER_OPACITY)
            } else {
                c::FG_DIM()
            }
        } else {
            c::BG_HOVER()
        },
    )
    .debug_selector(move || id.into())
    .size(rpx(CHROME_CONTROL_H))
    .rounded_full()
    .tab_index(0)
    .tab_stop(!disabled)
    .when(primary, |button| button.bg(fill))
    .aria_description(if disabled { "Unavailable" } else { label })
    .when(!disabled, |button| {
        button.focus_visible(|style| style.border_1().border_color(c::SEL_RING()))
    })
    .when(disabled, |button| {
        button.cursor_default().opacity(OPACITY_DISABLED)
    })
    .child(crate::icons::icon(
        icon,
        ICON_SM,
        if primary { c::BG() } else { c::FG_DIM() },
    ))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ComposerMenu {
    Settings,
    Model,
    Effort,
    Access,
}

/// A drag belongs to the catalog and session that were visible at pointer-down.
struct EffortDrag {
    scope: String,
    model: String,
    choices: Vec<String>,
    index: usize,
}

/// Presentation can move between stops without inventing a provider effort.
struct EffortVisual {
    scope: String,
    model: String,
    choices: Vec<String>,
    from: f32,
    target: f32,
    started: Option<std::time::Instant>,
    duration: std::time::Duration,
}

impl EffortVisual {
    fn position(&self, now: std::time::Instant) -> f32 {
        let Some(started) = self.started else {
            return self.target;
        };
        let elapsed = now.saturating_duration_since(started);
        if elapsed >= self.duration {
            return self.target;
        }
        let progress =
            super::motion::ease_standard(elapsed.as_secs_f32() / self.duration.as_secs_f32());
        self.from + (self.target - self.from) * progress
    }

    fn active(&self, now: std::time::Instant) -> bool {
        self.started
            .is_some_and(|started| now.saturating_duration_since(started) < self.duration)
    }
}

fn effort_fraction(index: usize, count: usize) -> f32 {
    if count == 1 {
        0.5
    } else {
        index as f32 / count.saturating_sub(1).max(1) as f32
    }
}

fn composer_scope(state: &codex::Snapshot) -> String {
    format!(
        "{:?}:{}:{:?}:{}:{:?}:{}",
        state.saved.provider,
        state.saved.key,
        state.saved.thread_id,
        state.saved.cwd,
        state.saved.config_args,
        state.saved.native_config_version
    )
}

fn composer_model_name(state: &codex::Snapshot) -> String {
    effective_model(state)
        .and_then(|model| model["displayName"].as_str())
        .or(state.model.as_deref())
        .or(state.resolved_model.as_deref())
        .unwrap_or("Configured model")
        .to_owned()
}

fn speed_disabled_explanation(reason: &str) -> &str {
    match reason {
        "extra_usage_disabled" => "paid usage credits are not enabled",
        "free" => "this account is not eligible",
        "preference" => "account or organization settings disable Fast",
        "not_first_party" => "Fast requires the Anthropic service",
        "disabled_by_env" => "the CLI environment disables Fast",
        "model_not_allowed" => "this model cannot use Fast",
        "sdk_opt_in_required" => "this session has not enabled Fast",
        "pending" => "Claude is checking availability",
        "network_error" => "the availability check could not connect",
        "unknown" => "Claude did not provide a reason",
        other => other,
    }
}

fn effort_label(value: &str) -> String {
    match value {
        "xhigh" => "Extra high".into(),
        _ => {
            let mut letters = value.chars();
            letters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(letters).collect()
            })
        }
    }
}

fn effort_stop(position: f32, count: usize) -> usize {
    (position.clamp(0.0, 1.0) * count.saturating_sub(1) as f32).round() as usize
}

fn effort_key_index(key: &str, index: usize, count: usize) -> Option<usize> {
    if count < 2 {
        return None;
    }
    let last = count - 1;
    match key {
        "left" | "down" => Some(index.saturating_sub(1)),
        "right" | "up" => Some((index + 1).min(last)),
        "home" => Some(0),
        "end" => Some(last),
        _ => None,
    }
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
    policy_expanded: bool,
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
    echo_text: Option<String>,
    echo_after: usize,
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
        "{}:{}:{}:{}",
        state.interaction_epoch,
        state
            .interaction_owners
            .get(&request.id.to_string())
            .map_or_else(
                || state.saved.thread_id.as_deref().unwrap_or_default(),
                |owner| owner.thread.as_str()
            ),
        if request.turn_bound() {
            state
                .interaction_owners
                .get(&request.id.to_string())
                .map_or_else(
                    || state.turn_id.as_deref().unwrap_or_default(),
                    |owner| owner.turn.as_deref().unwrap_or_default(),
                )
        } else {
            ""
        },
        request.id
    )
}
/// Provider text stays literal and selectable; explicit blocks preserve every line break.
fn literal_notice_text(id: impl Into<gpui::SharedString>, text: &str) -> impl IntoElement {
    let id: gpui::SharedString = id.into();
    div()
        .id(id.clone())
        .w_full()
        .min_w_0()
        .font_family(crate::fonts::UI_FAMILY)
        .text_size(rpx(TEXT_SMALL))
        .line_height(rpx(CARD_LINE_SM_H))
        .flex()
        .flex_col()
        .children(text.split('\n').enumerate().map(|(index, line)| {
            div()
                .debug_selector({
                    let id = id.clone();
                    move || format!("{id}-line-{index}")
                })
                .w_full()
                .min_w_0()
                .min_h(rpx(CARD_LINE_SM_H))
                .child(
                    TextView::html(
                        gpui::SharedString::from(format!("{id}-line-{index}")),
                        line.replace('&', "&amp;")
                            .replace('<', "&lt;")
                            .replace('>', "&gt;"),
                    )
                    .selectable(true)
                    .font_family(crate::fonts::UI_FAMILY)
                    .text_size(rpx(TEXT_SMALL))
                    .line_height(rpx(CARD_LINE_SM_H))
                    .text_color(c::FG())
                    .w_full()
                    .min_w_0(),
                )
        }))
}
fn permission_heading(title: String, source: String) -> gpui::Div {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(rpx(SPACE_SM))
        .child(crate::icons::icon(
            StatusKind::NeedsInput.icon_name(),
            ICON_MD,
            StatusKind::NeedsInput.color(),
        ))
        .child(
            div()
                .min_w_0()
                .text_color(StatusKind::NeedsInput.color())
                .text_size(rpx(TEXT_TITLE))
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(title),
        )
        .child(
            div()
                .min_w_0()
                .ml_auto()
                .text_size(rpx(TEXT_SMALL))
                .text_color(c::FG_DIM())
                .child(source),
        )
}
fn permission_code(id: String, text: &str) -> impl IntoElement {
    TextView::html(
        gpui::SharedString::from(id),
        format!(
            "<pre>{}</pre>",
            text.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        ),
    )
    .selectable(true)
    .whitespace_normal()
    .font_family(crate::fonts::MONO_FAMILY)
    .text_size(rpx(TEXT_CODE))
    .w_full()
    .min_w_0()
}

/// Attached backing shared by completion choices and queued messages.
fn composer_tray_inset(window: &Window) -> f32 {
    if window.viewport_size().width < gpui::px(CHAT_MAX_W) {
        SPACE_2XL
    } else {
        SPACE_20
    }
}
fn composer_tray(window: &Window) -> gpui::Div {
    div()
        .mx(rpx(composer_tray_inset(window)))
        .mb(-rpx(SPACE_2XL + SPACE_MD))
        .pb(rpx(SPACE_2XL + SPACE_MD))
        .px(rpx(SPACE_2XL))
        .pt(rpx(SPACE_LG))
        .rounded_t(rpx(RADIUS_PANEL))
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
    optimistic: Option<serde_json::Value>,
}

struct QueueEditor {
    id: u64,
    draft: String,
    images: Vec<String>,
    tokens: Vec<ComposerToken>,
    reference: Option<codex::ReferenceDraft>,
    loaded: bool,
    saving: bool,
}
struct QueueMotionRow {
    prompt: codex::QueuedPrompt,
    started: std::time::Instant,
    exiting: bool,
}

struct MessageEditor {
    _subscription: Subscription,
    index: usize,
    message_id: String,
    reference: Option<codex::ReferenceDraft>,
    input: Entity<InputState>,
    images: Vec<String>,
    restored: bool,
    accepted_prompts: Option<u64>,
    thread_id: Option<String>,
    send_bounds: Rc<Cell<gpui::Bounds<gpui::Pixels>>>,
    source_anchor: gpui::ScrollAnchor,
}

fn closed_shell_fence(fence: &str) -> bool {
    let mut lines = fence.lines();
    let first = lines.next().unwrap_or("").trim_start();
    let marker = if first.starts_with("```") { '`' } else { '~' };
    let count = first.chars().take_while(|c| *c == marker).count();
    count >= 3
        && lines.last().is_some_and(|last| {
            last.trim().chars().count() >= count && last.trim().chars().all(|c| c == marker)
        })
}

fn snippet_shell_supported(language: &str) -> bool {
    let shell = grove_core::env_path::login_shell();
    let shell = std::path::Path::new(&shell)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match language {
        "sh" => matches!(shell.as_str(), "sh" | "bash" | "zsh"),
        "bash" => shell == "bash",
        "zsh" => shell == "zsh",
        "powershell" | "pwsh" => matches!(shell.as_str(), "powershell" | "pwsh"),
        _ => false,
    }
}

pub(crate) enum CodexViewEvent {
    DashboardSubmit,
    ScriptLaunched,
}
impl gpui::EventEmitter<CodexViewEvent> for CodexView {}

pub struct CodexView {
    pub(crate) dashboard_tray: Option<Entity<super::sidebar::dashboard::DestinationTray>>,
    dashboard_pending: bool,
    dashboard_retry_focus: FocusHandle,
    script_runtime: Option<Entity<crate::runtime::Runtime>>,
    preview_child: Option<String>,
    session: Entity<CodexSession>,
    pub(super) html: Entity<super::inline_html::HtmlSession>,
    markdown_decorations: TextDecorationCollection,
    markdown_source: Option<(String, u64, Vec<TextDecoration>)>,
    native_plans: HashMap<(String, String), Entity<super::native_plan::NativePlan>>,
    input: Entity<InputState>,
    reference: Option<codex::ReferenceDraft>,
    submitted_reference: Option<String>,
    reference_expanded: bool,
    reference_disclosure_focus: FocusHandle,
    reference_remove_focus: FocusHandle,
    selection_views: HashMap<String, Entity<TextViewState>>,
    selection_dismissed: Option<TextSelectionSnapshot>,
    selection_root_observed: bool,
    selection_reply_focus: FocusHandle,
    submitted_images: Vec<String>,
    pub(super) transcript_bounds: Rc<Cell<gpui::Bounds<gpui::Pixels>>>,
    checkout_context: Option<(String, String)>,
    expanded: HashSet<String>,
    pub(super) agents_in_checkout: bool,
    work_tick_pending: Rc<Cell<bool>>,
    question_drafts: HashMap<String, QuestionGroupDraft>,
    question_receipts: Vec<QuestionReceipt>,
    question_generation: u64,
    approval_drafts: HashMap<String, ApprovalDraft>,
    elicitation_drafts: HashMap<String, ElicitationDraft>,
    interaction_scope: String,
    menu: Option<ComposerMenu>,
    popup: Option<Entity<PopupMenu>>,
    popup_subscription: Option<Subscription>,
    composer_menu_scope: Option<String>,
    composer_menu_launch_config: Option<(bool, bool)>,
    model_from_card: bool,
    model_picker: Option<Entity<model_picker::ModelPicker>>,
    effort_drag: Option<EffortDrag>,
    effort_visual: Option<EffortVisual>,
    effort_pending: Option<(String, String)>,
    effort_bounds: Rc<Cell<gpui::Bounds<gpui::Pixels>>>,
    card_model_focus: FocusHandle,
    card_focus: FocusHandle,
    card_reset_focus: FocusHandle,
    speed_focus: [FocusHandle; 3],
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
    message_editor: Option<MessageEditor>,
    edit_confirmation: Option<(usize, String)>,
    edit_request_pending: bool,
    edited_messages: u64,
    edit_cancel_focus: FocusHandle,
    edit_confirm_focus: FocusHandle,
    queue_resume_focus: FocusHandle,
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
    preview_follow_latest: bool,
    last_content: Option<[u64; 2]>,
    smooth_follow: bool,
    last_follow_offset: Option<gpui::Pixels>,
    last_follow_max: gpui::Pixels,
    motion_frame: Option<std::time::Instant>,
    motion_frame_pending: Rc<Cell<bool>>,
    stream_text: HashMap<String, (String, usize)>,
    message_hover: HashSet<String>,
    message_focus: HashMap<String, FocusHandle>,
    copied_message: Option<(String, u64)>,
    copied_code_blocks: HashMap<String, u64>,
    copy_generation: u64,
    timestamp_tick_pending: Rc<Cell<bool>>,
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
    tasks_expanded: bool,
    task_cache: Option<Rc<TaskCache>>,
    transcript_cache: Option<Rc<TranscriptRenderCache>>,
    subscriptions: Vec<Subscription>,
}
impl CodexView {
    fn sync_native_plans(&mut self, cx: &mut Context<Self>) {
        let plan_keys: HashSet<_> = self
            .session
            .read(cx)
            .state
            .saved
            .rich_plans
            .revisions
            .iter()
            .map(|p| (p.document.plan_id.clone(), p.document.revision_id.clone()))
            .collect();
        self.native_plans.retain(|key, _| plan_keys.contains(key));
        for (plan_id, revision_id) in plan_keys {
            let key = (plan_id.clone(), revision_id.clone());
            if !self.native_plans.contains_key(&key) {
                let session = self.session.clone();
                let html = self.html.clone();
                let view = cx.new(|cx| {
                    super::native_plan::NativePlan::new(session, html, plan_id, revision_id, cx)
                });
                self.subscriptions.push(cx.observe(&view, |this, _, cx| {
                    this.smooth_follow = false;
                    cx.notify();
                }));
                self.native_plans.insert(key, view);
            }
        }
    }

    /// Latest immutable revision of each plan, in conversation order.
    pub(super) fn native_plans_for_sidebar(
        &self,
        cx: &App,
    ) -> Vec<Entity<super::native_plan::NativePlan>> {
        let mut seen = HashSet::new();
        let mut plans: Vec<_> = self
            .session
            .read(cx)
            .state
            .saved
            .rich_plans
            .revisions
            .iter()
            .rev()
            .filter(|r| seen.insert(r.document.plan_id.clone()))
            .filter_map(|r| {
                self.native_plans
                    .get(&(r.document.plan_id.clone(), r.document.revision_id.clone()))
                    .cloned()
            })
            .collect();
        plans.reverse();
        plans
    }

    pub(super) fn child_preview(
        session: Entity<CodexSession>,
        child: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::new(session, window, cx);
        view.preview_child = Some(child);
        view
    }

    pub fn new(session: Entity<CodexSession>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let stream_thread = session.read(cx).state.saved.thread_id.clone();
        let draft = session.read(cx).state.saved.draft.clone();
        let images = session.read(cx).state.saved.draft_images.clone();
        let reference = session.read(cx).state.saved.draft_reference.clone();
        let previews = images
            .iter()
            .filter_map(|url| image_from_url(url).map(|image| (url.clone(), image)))
            .collect();
        let placeholder = composer_placeholder(session.read(cx).state.saved.provider);
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .auto_grow(3, 10)
                .accept_image_paste(true)
                .markdown_auto_continue(true)
                .submit_on_enter(true)
                .placeholder(placeholder)
                .default_value(draft)
        });
        let markdown_decorations = input.update(cx, |input, cx| {
            input.create_decorations_collection(Vec::new(), cx)
        });
        let card_focus = cx.focus_handle();
        let mut subscriptions = vec![
            cx.observe(&session, |this, _, cx| {
                this.sync_native_plans(cx);
                if this.menu.is_some() && !this.composer_menu_valid(cx) {
                    this.close_menu(cx);
                }
                cx.notify();
            }),
            cx.observe(&input, |this, _, cx| {
                this.validate_picked_tokens(cx);
                this.refresh_files(cx);
                cx.notify();
            }),
            cx.subscribe_in(&input, window, |this, _, event, window, cx| {
                if this.preview_child.is_some() {
                    return;
                }
                match event {
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
                    InputEvent::PasteImages(images) => {
                        this.paste_images(images.clone(), window, cx);
                    }
                    InputEvent::Focus => {
                        crate::platform::inline_html::NativeDocument::focus_host(window);
                        this.set_composer_collapsed(false, cx);
                    }
                    InputEvent::Change => {
                        this.set_composer_collapsed(false, cx);
                        let text = this.input.read(cx).value().to_string();
                        if this.preview_child.is_none() && this.queue_editor.is_none() {
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
                }
            }),
        ];
        let scroll = gpui::ScrollHandle::new();
        let html_scroll = scroll.clone();
        let cwd = std::path::PathBuf::from(&session.read(cx).state.saved.cwd);
        let html = cx.new(|_| super::inline_html::HtmlSession::new(cwd, html_scroll));
        subscriptions.push(cx.observe(&html, |this, _, cx| {
            this.smooth_follow = false;
            this.follow_after_submit = false;
            this.composer_pin_bottom = false;
            cx.notify();
        }));
        subscriptions.push(cx.on_focus_out(&card_focus, window, |this, _, _, cx| {
            if this.menu == Some(ComposerMenu::Settings) {
                this.close_menu(cx);
            }
        }));
        let mut view = Self {
            preview_child: None,
            session,
            html,
            native_plans: HashMap::new(),
            markdown_decorations,
            markdown_source: None,
            input,
            reference,
            submitted_reference: None,
            reference_expanded: false,
            reference_disclosure_focus: cx.focus_handle(),
            reference_remove_focus: cx.focus_handle(),
            selection_views: HashMap::new(),
            selection_dismissed: None,
            selection_root_observed: false,
            selection_reply_focus: cx.focus_handle(),
            submitted_images: Vec::new(),
            transcript_bounds: Rc::default(),
            expanded: HashSet::new(),
            agents_in_checkout: false,
            work_tick_pending: Rc::new(Cell::new(false)),
            question_drafts: HashMap::new(),
            question_receipts: Vec::new(),
            question_generation: 0,
            approval_drafts: HashMap::new(),
            elicitation_drafts: HashMap::new(),
            interaction_scope: String::new(),
            menu: None,
            popup: None,
            popup_subscription: None,
            composer_menu_scope: None,
            composer_menu_launch_config: None,
            model_from_card: false,
            model_picker: None,
            effort_drag: None,
            effort_visual: None,
            effort_pending: None,
            effort_bounds: Rc::default(),
            card_model_focus: cx.focus_handle(),
            card_focus,
            card_reset_focus: cx.focus_handle(),
            speed_focus: std::array::from_fn(|_| cx.focus_handle()),
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
            message_editor: None,
            edit_confirmation: None,
            edit_request_pending: false,
            edited_messages: 0,
            edit_cancel_focus: cx.focus_handle(),
            edit_confirm_focus: cx.focus_handle(),
            queue_resume_focus: cx.focus_handle().tab_stop(true),
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
            scroll,
            preview_follow_latest: true,
            last_content: None,
            smooth_follow: false,
            last_follow_offset: None,
            last_follow_max: gpui::Pixels::ZERO,
            motion_frame: None,
            motion_frame_pending: Rc::default(),
            stream_text: HashMap::new(),
            message_hover: HashSet::new(),
            message_focus: HashMap::new(),
            copied_message: None,
            copied_code_blocks: HashMap::new(),
            copy_generation: 0,
            timestamp_tick_pending: Rc::new(Cell::new(false)),
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
            tasks_expanded: true,
            task_cache: None,
            transcript_cache: None,
            dashboard_tray: None,
            dashboard_pending: false,
            dashboard_retry_focus: cx.focus_handle(),
            script_runtime: None,
            checkout_context: None,
            subscriptions,
        };
        view.sync_native_plans(cx);
        view
    }
    pub(crate) fn set_script_runtime(
        &mut self,
        runtime: Entity<crate::runtime::Runtime>,
        _cx: &mut Context<Self>,
    ) {
        self.script_runtime = Some(runtime);
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
                        [
                            ("folder", identity),
                            ("rail-tree", location),
                            ("git-branch", branch),
                        ]
                        .into_iter()
                        .filter(|(_, value)| !value.is_empty())
                        .enumerate()
                        .map(|(index, (glyph, value))| {
                            let full_value = value.clone();
                            div()
                                .id(("codex-launch-identity", index))
                                .min_w_0()
                                .max_w_full()
                                .flex_shrink_0()
                                .whitespace_normal()
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_SM))
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
                                .child(crate::icons::icon(glyph, ICON_SM, c::FG_DIM()))
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
            (
                "codex-starter-understand",
                "Understand this project",
                "Map the code and explain where to start",
                "Explain the main modules in this project and where I should start.",
            ),
            (
                "codex-starter-change",
                "Make a change",
                "Describe what should work differently",
                "Help me make a change in this project. Ask me what should work differently before editing.",
            ),
            (
                "codex-starter-review",
                "Review my changes",
                "Look for bugs in the current diff",
                "Review my uncommitted changes for bugs and regressions.",
            ),
        ] {
            starters = starters.child(
                header_control(id, title)
                    .debug_selector(move || id.into())
                    .tab_index(0)
                    .flex_1()
                    .flex_basis(rpx(CHAT_MAX_W / 3.0 - SPACE_LG))
                    .min_w_0()
                    .h_auto()
                    .p(rpx(SPACE_2XL))
                    .border_1()
                    .border_color(c::BORDER_SOFT())
                    .rounded(rpx(RADIUS_CONTROL))
                    .flex()
                    .flex_col()
                    .items_start()
                    .justify_start()
                    .gap(rpx(SPACE_SM))
                    .focus_visible(|style| style.bg(c::BG_HOVER()).border_color(c::BORDER_STRONG()))
                    .aria_description(format!(
                        "{description}. Adds an editable prompt without sending."
                    ))
                    .child(
                        div()
                            .id(gpui::SharedString::from(format!("{id}-title")))
                            .debug_selector(move || format!("{id}-title"))
                            .text_color(c::FG())
                            .child(title),
                    )
                    .child(
                        div()
                            .whitespace_normal()
                            .text_color(c::FG_DIM())
                            .child(description),
                    )
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            if action_key(event) {
                                this.use_starter(prompt, window, cx);
                                cx.stop_propagation();
                            }
                        },
                    ))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.use_starter(prompt, window, cx);
                    })),
            );
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
            let steering = pending
                .optimistic
                .as_ref()
                .is_some_and(|item| item["localSteering"] == true);
            let queued_steering = pending
                .optimistic
                .as_ref()
                .is_some_and(|item| item["localQueuedSteering"] == true);
            let steering_pending = steering
                && (state
                    .queued_prompts
                    .iter()
                    .any(|prompt| prompt.id == pending.submission)
                    || (!queued_steering && state.accepted_submission != Some(pending.submission)));
            if let Some(optimistic) = &mut pending.optimistic {
                optimistic["localSteering"] = steering_pending.into();
            }
            pending.existing.extend(consumed.iter().cloned());
            let id = state.saved.items.iter().find_map(|item| {
                let id = item["id"].as_str()?;
                (item["type"] == "userMessage"
                    && !pending.existing.contains(id)
                    && !consumed.contains(id)
                    && codex::item_text(item) == pending.text
                    && pending.optimistic.as_ref().is_none_or(|optimistic| {
                        let images = |message: &serde_json::Value| {
                            message["content"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|part| {
                                    part["url"].as_str().or_else(|| part["image_url"].as_str())
                                })
                                .map(str::to_owned)
                                .collect::<Vec<_>>()
                        };
                        images(item) == images(optimistic)
                    }))
                .then_some((id, item))
            });
            if let Some((id, item)) = id {
                consumed.insert(id.to_owned());
                if let Some(optimistic) = &pending.optimistic {
                    if let Some(animation) = self
                        .animating_bubbles
                        .remove(optimistic["id"].as_str().unwrap_or_default())
                    {
                        self.animating_bubbles.insert(id.to_owned(), animation);
                    }
                } else if !reduced_motion {
                    self.animating_bubbles
                        .insert(id.to_owned(), (now, Rc::new(Cell::new(false))));
                }
                if steering_pending {
                    let mut echo = item.clone();
                    echo["localSteering"] = true.into();
                    echo["localQueuedSteering"] = queued_steering.into();
                    pending.optimistic = Some(echo);
                    true
                } else {
                    false
                }
            } else {
                true
            }
        });
    }
    fn record_submission_bubble(
        &mut self,
        submission: u64,
        text: String,
        images: Vec<String>,
        steering: bool,
        queued_steering: bool,
        cx: &mut Context<Self>,
    ) {
        let id = format!("local-submission-{submission}");
        if !cx.reduce_motion() {
            self.animating_bubbles.insert(
                id.clone(),
                (cx.background_executor().now(), Rc::new(Cell::new(false))),
            );
        }
        let mut content = vec![serde_json::json!({"type":"text", "text":text})];
        content.extend(
            images
                .iter()
                .map(|url| serde_json::json!({"type":"image", "url":url})),
        );
        let state = &self.session.read(cx).state;
        self.bubble_entrances.push(BubbleEntrance {
            submission,
            existing: state.saved.items.iter().filter_map(|item| item["id"].as_str().map(str::to_owned)).collect(),
            thread: state.saved.thread_id.clone(),
            text,
            optimistic: Some(serde_json::json!({"id":id, "type":"userMessage", "content":content, "localSteering":steering, "localQueuedSteering":queued_steering, "groveReceivedAt":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64})),
        });
        self.follow_after_submit = true;
    }
    fn queue_action(&mut self, action: Action, cx: &mut Context<Self>) -> bool {
        let steering = if let Action::SteerQueued(id) = &action {
            self.session
                .read(cx)
                .state
                .queued_prompts
                .iter()
                .find(|prompt| prompt.id == *id)
                .cloned()
        } else {
            None
        };
        match self.session.read(cx).handle.send(action) {
            Ok(()) => {
                if let Some(prompt) = steering {
                    self.bubble_entrances
                        .retain(|pending| pending.submission != prompt.id);
                    self.record_submission_bubble(
                        prompt.id,
                        prompt.text,
                        prompt.images,
                        true,
                        true,
                        cx,
                    );
                    cx.notify();
                }
                true
            }
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
            reference: None,
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
        let draft = self.input.read(cx).value().to_string();
        let text = editor
            .reference
            .as_ref()
            .map_or_else(|| draft.clone(), |reference| reference.serialize(&draft));
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
                    editor.images.clone_from(&self.images);
                    editor.tokens.clone_from(&self.picked_tokens);
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
                    let text = if let Some((reference, reply)) = reference_prompt(&prompt.text) {
                        editor.reference = Some(reference);
                        reply.to_owned()
                    } else {
                        prompt.text.clone()
                    };
                    self.input.update(cx, |input, cx| {
                        input.set_value(text, window, cx);
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
        let steering_ids: HashSet<_> = self
            .bubble_entrances
            .iter()
            .filter(|pending| {
                pending
                    .optimistic
                    .as_ref()
                    .is_some_and(|item| item["localSteering"] == true)
            })
            .map(|pending| pending.submission)
            .collect();
        if self
            .queue_motion_rows
            .iter()
            .all(|row| steering_ids.contains(&row.prompt.id))
        {
            return div().into_any_element();
        }
        let count = state
            .queued_prompts
            .iter()
            .filter(|prompt| !steering_ids.contains(&prompt.id))
            .count();
        let pending_count = count;
        let resume_enabled =
            state.status != codex::Status::Disconnected && state.queue_uncertain.is_empty();
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
                            .child("Queued"),
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
                                .debug_selector(|| "codex-resume-queue".into())
                                .when(!resume_enabled, gpui::Styled::cursor_default)
                                .track_focus(
                                    &self
                                        .queue_resume_focus
                                        .clone()
                                        .tab_stop(resume_enabled)
                                        .tab_index(0),
                                )
                                .focus_visible(|style| style.bg(c::BG_HOVER()))
                                .child(crate::icons::icon("arrow-up", ICON_MD, c::FG_DIM()))
                                .on_key_down(cx.listener(
                                    move |this, event: &gpui::KeyDownEvent, _, cx| {
                                        if resume_enabled && action_key(event) {
                                            this.queue_action(Action::ResumeQueue, cx);
                                            cx.stop_propagation();
                                        }
                                    },
                                ))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if resume_enabled {
                                        this.queue_action(Action::ResumeQueue, cx);
                                    }
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
                if steering_ids.contains(&id) {
                    continue;
                }
                let exiting = motion_row.exiting;
                let locked = exiting || !state.queue_editable(id) || self.queue_editor.is_some();
                let steer = !locked && active_turn_redirect_available(state);
                let selected = self.queue_selected == Some(id);
                let label = if prompt.images.is_empty() {
                    collapsed_draft_preview(&reference_prompt_copy(&prompt.text))
                } else {
                    format!(
                        "{} · {} images",
                        collapsed_draft_preview(&reference_prompt_copy(&prompt.text)),
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
                        form_action("codex-dismiss-uncertain", "Dismiss", false, window, cx).icon(gpui_component::Icon::default().path("icons/close.svg").size(rpx(ICON_SM)))
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
                            .when(locked, gpui::Styled::cursor_default)
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
                    .child(
                        header_control("codex-steer-queued", STEER_DESCRIPTION)
                            .w_auto()
                            .px(rpx(SPACE_SM))
                            .tab_index(0)
                            .tab_stop(steer)
                            .when(!steer, gpui::Styled::cursor_default)
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
                                    .child(STEER_LABEL),
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
                    .child(
                        header_control("codex-remove-queued", "Remove queued message")
                            .tab_index(0)
                            .tab_stop(!locked)
                            .when(locked, gpui::Styled::cursor_default)
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
                            .when(saving, gpui::Styled::cursor_default)
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
                            .when(saving, gpui::Styled::cursor_default)
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
            || (self.dashboard_tray.is_none() && state.status == codex::Status::Disconnected)
            || (self.dashboard_tray.is_none()
                && state.status == codex::Status::Starting
                && state.saved.provider != codex::Provider::Claude)
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
        let min_rows = if self.dashboard_tray.is_some() { 2 } else { 3 };
        self.input.update(cx, |input, cx| {
            if collapsed {
                input.set_auto_grow(1, 1, cx);
            } else {
                input.set_auto_grow(min_rows, 10, cx);
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

    fn toggle_tasks(&mut self, cx: &mut Context<Self>) {
        if self.composer_collapsed {
            self.set_composer_collapsed(false, cx);
            self.tasks_expanded = true;
        } else {
            self.tasks_expanded = !self.tasks_expanded;
        }
        cx.notify();
    }

    /// Current session feedback shares the task dock’s exact horizontal inset.
    fn session_notice(
        &self,
        state: &codex::Snapshot,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let disconnected = state.status == codex::Status::Disconnected;
        let notice = state.effective_notice();
        if !disconnected && notice.is_some_and(|n| n.kind == codex::SessionNoticeKind::Retrying) {
            return None;
        }
        let body = state
            .error
            .as_deref()
            .or(disconnected.then_some("The session is disconnected."))?;
        let title = if disconnected {
            "Connection lost"
        } else if notice.is_some_and(|n| n.kind == codex::SessionNoticeKind::UsageLimit) {
            "Usage limit reached"
        } else if state.status.busy() {
            "Provider error"
        } else {
            "Request stopped"
        };
        let notice_kind = if notice.is_some_and(|n| n.kind == codex::SessionNoticeKind::UsageLimit)
        {
            StatusKind::Warning
        } else {
            StatusKind::Failed
        };
        let expanded = self.expanded.contains("session-notice-details");
        let (summary, details) =
            if notice.is_some_and(|n| n.kind == codex::SessionNoticeKind::UsageLimit) {
                (
                    "The provider’s usage limit prevents this request from continuing.",
                    Some(body),
                )
            } else if notice.is_some() {
                body.split_once('\n')
                    .map_or((body, None), |(first, rest)| (first, Some(rest)))
            } else {
                (body, None)
            };
        let mut content = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_MD))
            .when(disconnected || notice.is_some(), |content| {
                content.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(notice_kind.color())
                        .child(title),
                )
            })
            .child(literal_notice_text("session-notice-body", summary));
        if let Some(reset) = notice.and_then(|n| n.reset_at) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let seconds = reset.saturating_sub(now).max(0);
            let reset_text = if seconds == 0 {
                "The reported reset time has passed.".to_owned()
            } else if seconds < 60 {
                "Usage resets in less than a minute.".to_owned()
            } else if seconds < 3600 {
                format!("Usage resets in {} minutes.", (seconds + 59) / 60)
            } else {
                format!("Usage resets in about {} hours.", (seconds + 3599) / 3600)
            };
            content = content.child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(reset_text),
            );
        }
        let copy_body = body.to_owned();
        let mut actions = div().flex().flex_wrap().gap(rpx(SPACE_SM)).child(
            form_action("session-notice-copy", "Copy message", false, window, cx)
                .icon(
                    gpui_component::Icon::default()
                        .path("icons/copy.svg")
                        .size(rpx(ICON_SM)),
                )
                .debug_selector(|| "session-notice-copy".into())
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_body.clone()));
                }),
        );
        if disconnected {
            actions = actions.child(
                form_action("codex-reconnect", "Reconnect", false, window, cx)
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/restart.svg")
                            .size(rpx(ICON_SM)),
                    )
                    .debug_selector(|| "codex-reconnect".into())
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.session.read(cx).state.status == codex::Status::Disconnected {
                            this.session
                                .update(cx, |session, cx| session.send(Action::Reconnect, cx));
                        }
                    })),
            );
        } else if let Some(url) = notice.and_then(|n| n.usage_url) {
            actions = actions.child(
                form_action("session-notice-usage", "Manage usage", false, window, cx)
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/external-link.svg")
                            .size(rpx(ICON_SM)),
                    )
                    .on_click(move |_, _, cx| cx.open_url(url)),
            );
        }
        if details.is_some() {
            actions = actions.child(
                form_action(
                    "session-notice-details",
                    if expanded {
                        "Hide details"
                    } else {
                        "Show details"
                    },
                    false,
                    window,
                    cx,
                )
                .icon(
                    gpui_component::Icon::default()
                        .path(if expanded {
                            "icons/chev-down.svg"
                        } else {
                            "icons/chev-right.svg"
                        })
                        .size(rpx(ICON_SM)),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if !this.expanded.remove("session-notice-details") {
                        this.expanded.insert("session-notice-details".into());
                    }
                    cx.notify();
                })),
            );
        }
        content = content.child(actions);
        if expanded {
            if let Some(details) = details {
                content = content.child(
                    div()
                        .id("session-notice-diagnostic-scroll")
                        .max_h(rpx(TOOL_DETAIL_MAX_H))
                        .overflow_y_scroll()
                        .child(literal_notice_text("session-notice-diagnostic", details)),
                );
            }
        }
        Some(
            div()
                .id("codex-error")
                .debug_selector(|| "codex-error".into())
                .role(gpui::Role::Alert)
                .flex_shrink_0()
                .mx(rpx(composer_tray_inset(window)))
                .p(rpx(SPACE_2XL))
                .rounded(rpx(RADIUS_PANEL))
                .border_1()
                .border_color(c::BORDER_SOFT())
                .bg(c::BG_RAIL())
                .flex()
                .gap(rpx(SPACE_LG))
                .child(status::mark(
                    "session-notice-mark",
                    notice_kind,
                    ICON_MD,
                    cx,
                ))
                .child(content)
                .into_any_element(),
        )
    }

    /// The docked, read-only checklist; jumps identify the task’s transcript start.
    fn task_strip(
        &self,
        list: &TaskList,
        status: codex::Status,
        jumps: Vec<usize>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // Browsing temporarily compacts both docked surfaces without changing
        // the user's task disclosure preference.
        let expanded = self.tasks_expanded && !self.composer_collapsed;
        let phase = task_phase(status);
        let (done, total) = (list.done(), list.total());
        let summary = task_summary(list, phase);
        let header_status = task_header_status(list);
        let separator = || div().flex_shrink_0().text_color(c::FG_MUTE()).child("·");
        let note = match (&summary, expanded) {
            // Expanded, the rows carry the step; the header keeps only the state.
            (TaskSummary::Waiting(_), true) => Some(
                div()
                    .flex_shrink_0()
                    .text_color(StatusKind::NeedsInput.color())
                    .child("Waiting for you")
                    .into_any_element(),
            ),
            (TaskSummary::Stopped(_), true) => Some(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(summary.text())
                    .into_any_element(),
            ),
            (_, true) | (TaskSummary::None, false) => None,
            // Trailing notes grow into the slack: GPUI truncates by summed glyph
            // advances, which exceed a content-sized box's kerned width. The
            // step therefore trails the fixed "Waiting for you".
            (TaskSummary::Waiting(step), false) => Some(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(StatusKind::NeedsInput.color())
                            .child("Waiting for you"),
                    )
                    .child(separator())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(c::FG())
                            .child(step.clone()),
                    )
                    .into_any_element(),
            ),
            (TaskSummary::Now(label), false) => Some(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(c::FG())
                    .child(label.clone())
                    .into_any_element(),
            ),
            (TaskSummary::AllDone(_) | TaskSummary::Stopped(_), false) => Some(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(summary.text())
                    .into_any_element(),
            ),
        };
        let progress = format!("{done} of {total} done");
        let cluster = div()
            .id("codex-task-strip-summary")
            .debug_selector(|| "codex-task-strip-summary".into())
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(rpx(SPACE_LG))
            .child(animated_task_mark(
                "codex-task-strip-mark",
                header_status,
                task_mark_phase(header_status, phase),
                cx,
            ))
            .child(div().flex_shrink_0().text_color(c::FG_DIM()).child("Tasks"))
            .child(
                div()
                    .flex_shrink_0()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(format!("{done}/{total}")),
            )
            .child(task_meter(done, total, ICON_BTN_W))
            .when_some(note, |cluster, note| cluster.child(separator()).child(note));
        let header = div()
            .id("codex-task-strip")
            .debug_selector(|| "codex-task-strip".into())
            .role(gpui::Role::Button)
            .cursor_pointer()
            .aria_label(format!("Tasks, {progress}"))
            .aria_description(summary.text())
            .aria_expanded(expanded)
            .tab_index(0)
            .w_full()
            .h(rpx(TASK_STRIP_HEAD_H))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(rpx(SPACE_SM))
            .text_size(rpx(TEXT_SMALL))
            .text_color(c::FG_DIM())
            .whitespace_nowrap()
            .focus_visible(|style| style.bg(c::MENU_HOVER()))
            .child(crate::icons::icon(
                if expanded { "chev-down" } else { "chev-right" },
                ICON_MD,
                c::FG_MUTE(),
            ))
            .child(cluster)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if action_key(event) {
                    this.toggle_tasks(cx);
                    cx.stop_propagation();
                }
            }))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_tasks(cx)));
        let body = expanded.then(|| {
            let mut rows = div()
                .id("codex-task-list")
                .debug_selector(|| "codex-task-list".into())
                .flex_1()
                .min_h_0()
                .max_h(rpx(COMPOSER_MENU_MAX_H))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .when_some(list.explanation.clone(), |rows, explanation| {
                    rows.child(
                        div()
                            .id("codex-task-explanation")
                            .debug_selector(|| "codex-task-explanation".into())
                            .px(rpx(SPACE_LG))
                            .py(rpx(SPACE_SM))
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .whitespace_normal()
                            .child(explanation),
                    )
                });
            for (task_index, (task, child)) in list.tasks.iter().zip(jumps).enumerate() {
                let current = task.status == TaskStatus::InProgress;
                rows = rows.child(
                    div()
                        .id(("codex-task-row", task_index))
                        .debug_selector(move || format!("codex-task-row-{task_index}"))
                        .group("codex-task-row")
                        .relative()
                        .role(gpui::Role::Button)
                        .cursor_pointer()
                        .aria_label(format!("Jump to start: {}", task.step))
                        .tooltip({
                            let step = task.step.clone();
                            move |window, cx| tooltip(step.clone(), window).build(window, cx)
                        })
                        .tab_index(0)
                        .min_h(rpx(TASK_ROW_H))
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_LG))
                        .px(rpx(SPACE_LG))
                        .rounded(rpx(RADIUS_CONTROL))
                        .text_size(rpx(TEXT_SMALL))
                        .hover(|style| style.bg(c::MENU_HOVER()))
                        .focus_visible(|style| style.bg(c::MENU_HOVER()))
                        .child(animated_task_mark(
                            ("codex-task-row-mark", task_index),
                            task.status,
                            task_mark_phase(task.status, phase),
                            cx,
                        ))
                        .child(
                            div()
                                .debug_selector(move || format!("codex-task-title-{task_index}"))
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(if task.status == TaskStatus::Completed {
                                    c::FG_DIM()
                                } else {
                                    c::FG()
                                })
                                .when(current, |text| text.font_weight(gpui::FontWeight::MEDIUM))
                                .child(task.step.clone()),
                        )
                        .child(
                            div()
                                .id(("codex-task-jump", task_index))
                                .debug_selector(move || format!("codex-task-jump-{task_index}"))
                                // Reserve the action slot so revealing it never covers task text.
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_SM))
                                .px(rpx(SPACE_LG))
                                .rounded_r(rpx(RADIUS_GROUP))
                                .bg(c::MENU_HOVER())
                                .opacity(0.0)
                                .group_hover("codex-task-row", |style| style.opacity(1.0))
                                .text_size(rpx(TEXT_MICRO))
                                .text_color(c::FG_DIM())
                                .child(crate::icons::icon("arrow-up", ICON_SM, c::FG_DIM()))
                                .child("Jump to start"),
                        )
                        .on_key_down(cx.listener(
                            move |this, event: &gpui::KeyDownEvent, window, cx| {
                                if action_key(event) {
                                    this.jump_to_message(child, window, cx);
                                    cx.stop_propagation();
                                }
                            },
                        ))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.jump_to_message(child, window, cx);
                        })),
                );
            }
            rows
        });
        composer_tray(window)
            .id("codex-task-strip-panel")
            .debug_selector(|| "codex-task-strip-panel".into())
            .when(!expanded, |panel| panel.bg(c::BG()))
            .role(gpui::Role::Group)
            .aria_label(format!("Agent tasks, {progress}"))
            // Shrinks by default: in a short pane the strip yields height to
            // the composer, its list scrolls, and the header row stays visible.
            .min_h(rpx(TASK_STRIP_HEAD_H))
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(header)
                    .children(body),
            )
            .into_any_element()
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
                .cursor_pointer()
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
        if self.preview_child.is_some() {
            self.preview_follow_latest = toward_bottom
                && self.scroll.max_offset().y + self.scroll.offset().y <= gpui::px(1.);
            // Native dispatch applies the wheel delta after this listener. Recheck
            // the resulting offset next render without routing into composer browse.
            self.pending_composer_scroll = Some(toward_bottom);
            cx.notify();
            return;
        }
        self.pending_composer_scroll = Some(toward_bottom);
        self.composer_pin_bottom = false;
        cx.notify();
    }

    pub(crate) fn attach_dashboard_tray(
        &mut self,
        tray: Entity<super::sidebar::dashboard::DestinationTray>,
        cx: &mut Context<Self>,
    ) {
        self.dashboard_tray = Some(tray);
        self.input
            .update(cx, |input, cx| input.set_auto_grow(2, 10, cx));
    }

    pub(crate) fn launch_dashboard(&mut self, cx: &mut Context<Self>) {
        self.dashboard_tray = None;
        self.input
            .update(cx, |input, cx| input.set_auto_grow(3, 10, cx));
        self.dashboard_pending = true;
        cx.notify();
    }
    fn retry_dashboard_catalog(&mut self, cx: &mut Context<Self>) {
        if self.dashboard_tray.is_none() || !self.session.read(cx).is_deferred() {
            return;
        }
        let settings = &cx.global::<crate::settings::SettingsState>().store;
        let provider = self.session.read(cx).state.saved.provider;
        let args = provider.agent().launch_args_for_backend(
            true,
            settings.yolo_enabled,
            settings.chrome_enabled.unwrap_or(false),
        );
        self.session.update(cx, |session, cx| {
            session.refresh_catalog(session.state.saved.cwd.clone(), args, cx);
        });
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dashboard_tray.is_some() {
            let session = self.session.read(cx);
            if session.catalog_loading || session.catalog_error.is_some() {
                return;
            }
            if !self.input.read(cx).value().trim().is_empty() || !self.images.is_empty() {
                cx.emit(CodexViewEvent::DashboardSubmit);
            }
            return;
        }
        if self.message_editor.is_some()
            || self.edit_confirmation.is_some()
            || self.edit_request_pending
            || self.session.read(cx).state.saved.pending_edit.is_some()
        {
            return;
        }

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
            session.state.clear_error();
            session.send(Action::Interrupt, cx);
        });
        if self.session.read(cx).state.error.is_some() {
            self.interrupt_requested_for_turn = None;
        }
        cx.notify();
    }
    fn submit_mode(&mut self, mode: SubmissionMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.message_editor.is_some()
            || self.edit_confirmation.is_some()
            || self.edit_request_pending
            || self.session.read(cx).state.saved.pending_edit.is_some()
        {
            return;
        }
        if self.queue_editor.is_some() {
            self.save_queue_edit(cx);
            return;
        }
        if self.submitted.is_none() && self.run_command(window, cx) {
            return;
        }
        let state = &self.session.read(cx).state;
        if self.draft_action_pending(cx)
            || (mode == SubmissionMode::Steer && !active_turn_redirect_available(state))
        {
            return;
        }
        let draft = self.input.read(cx).value().to_string();
        let text = self
            .reference
            .as_ref()
            .map_or_else(|| draft.clone(), |quote| quote.serialize(&draft));
        if text.trim().is_empty() && self.images.is_empty() {
            return;
        }
        let result = self.session.update(cx, |session, cx| {
            codex::validate_prompt(&text, &self.images)?;
            session
                .handle
                .send(Action::DraftReference(self.reference.clone()))?;
            session.state.saved.draft.clone_from(&draft);
            session
                .state
                .saved
                .draft_reference
                .clone_from(&self.reference);
            session.submit_with_mode(text.clone(), self.images.clone(), mode, cx)
        });
        if let Ok(submission) = result {
            if mode != SubmissionMode::Queue {
                self.record_submission_bubble(
                    submission,
                    text.clone(),
                    self.images.clone(),
                    mode == SubmissionMode::Steer,
                    false,
                    cx,
                );
            } else {
                let state = &self.session.read(cx).state;
                self.bubble_entrances.push(BubbleEntrance {
                    submission,
                    existing: state
                        .saved
                        .items
                        .iter()
                        .filter_map(|item| item["id"].as_str().map(str::to_owned))
                        .collect(),
                    thread: state.saved.thread_id.clone(),
                    text: text.clone(),
                    optimistic: None,
                });
            }
            self.submitted_reference = self.reference.as_ref().map(|quote| quote.id.clone());
            self.submitted_images = self.images.clone();
            self.submitted = Some(draft);
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
        if !self.selection_root_observed {
            if let Some(root) = window.root::<gpui_component::Root>().flatten() {
                self.subscriptions
                    .push(cx.observe(&root, |_, _, cx| cx.notify()));
                self.selection_root_observed = true;
            }
        }
        if self.dashboard_pending
            && (self.session.read(cx).state.saved.provider == codex::Provider::Claude
                || self.session.read(cx).state.saved.thread_id.is_some())
            && self.session.read(cx).state.status == codex::Status::Idle
        {
            self.dashboard_pending = false;
            self.submit(window, cx);
        }
        let source = self.input.read(cx).value().to_string();
        let generation = grove_core::theme::generation();
        if self
            .markdown_source
            .as_ref()
            .is_none_or(|(previous, theme, _)| previous != &source || *theme != generation)
        {
            let decorations = composer_markdown_decorations(&source);
            let replacements = composer_markdown_replacements(&source);
            self.input.update(cx, |input, cx| {
                input.set_text_replacements(replacements, cx);
            });
            self.markdown_source = Some((source, generation, decorations));
        }
        // An identical programmatic replacement may adjust decorations even
        // though source bytes did not change. Restore cached spans as well.
        if let Some((_, _, decorations)) = &self.markdown_source {
            self.markdown_decorations.set(decorations.clone(), cx);
        }
        self.restore_image_tokens(window, cx);
        let state = if let Some(child) = &self.preview_child {
            Rc::new(self.session.read(cx).state.subagent_snapshot(child))
        } else {
            self.session.read(cx).state.shared()
        };
        // Source geometry belongs to one editor in one conversation, never to a
        // subsequently connected thread or another message at the same index.
        if self
            .message_editor
            .as_ref()
            .is_some_and(|editor| editor.thread_id != state.saved.thread_id)
        {
            self.message_editor = None;
            self.edit_confirmation = None;
        }
        if self
            .composer_menu_scope
            .as_ref()
            .is_some_and(|scope| *scope != composer_scope(&state))
        {
            self.close_menu(cx);
        }
        self.sync_native_plans(cx);
        // Each completed message points to its exact imported revision, even when
        // a file path is later reused or a previous revision is linked again.
        let native_links = state
            .saved
            .rich_plans
            .references
            .iter()
            .filter_map(|reference| {
                self.native_plans
                    .get(&(reference.plan_id.clone(), reference.revision_id.clone()))
                    .map(|view| {
                        (
                            (reference.message_id.clone(), reference.source.clone()),
                            view.downgrade(),
                        )
                    })
            })
            .collect();
        self.html
            .update(cx, |html, _| html.bind_native_plans(native_links));

        if state.edited_messages != self.edited_messages {
            self.edited_messages = state.edited_messages;
            self.edit_request_pending = false;
            if let Some(editor) = &mut self.message_editor {
                editor.restored = true;
            }
            self.send_inline_edit(cx);
            self.bubble_entrances.clear();
            self.animating_bubbles.clear();
            self.stream_text.clear();
            self.set_composer_collapsed(false, cx);
        }

        if state.error.is_some() {
            self.edit_request_pending = false;
            if let Some(editor) = &mut self.message_editor {
                editor.accepted_prompts = None;
            }
        }
        if self
            .message_editor
            .as_ref()
            .and_then(|editor| editor.accepted_prompts)
            .is_some_and(|accepted| state.accepted_prompts > accepted)
        {
            self.message_editor = None;
            window.focus(&self.input.focus_handle(cx), cx);
        }
        // checkout_context contains chrome labels such as “Main checkout”, not
        // filesystem paths. Resolve document links against the saved worktree.
        let html_cwd = std::path::PathBuf::from(&state.saved.cwd);
        let html_paused = self.menu.is_some()
            || self.popup.is_some()
            || !self.file_suggestions.is_empty()
            || self.edit_confirmation.is_some();
        self.html.update(cx, |html, cx| {
            html.sync(&state, html_cwd, html_paused, window, cx);
        });
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
            self.reference = None;
            self.submitted_reference = None;
            self.selection_views.clear();
            self.selection_dismissed = None;
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
        let scope = format!("{:?}:{}", state.saved.thread_id, state.interaction_epoch);
        if self.interaction_scope != scope {
            let restore_focus = self.approval_drafts.values().any(|draft| {
                draft.input.focus_handle(cx).is_focused(window)
                    || draft.feedback.focus_handle(cx).is_focused(window)
            }) || self.elicitation_drafts.values().any(|draft| {
                draft.fields.iter().any(|field| {
                    field.input.focus_handle(cx).is_focused(window)
                        || field
                            .choice_focus
                            .values()
                            .any(|focus| focus.is_focused(window))
                })
            });
            self.approval_drafts.clear();
            self.question_drafts.clear();
            self.question_receipts.clear();
            self.elicitation_drafts.clear();
            self.interaction_scope = scope;
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
            let reference_matches =
                self.reference.as_ref().map(|quote| &quote.id) == self.submitted_reference.as_ref();
            if reference_matches {
                self.reference = None;
                self.reference_expanded = false;
            }
            self.submitted_reference = None;
            let submitted = self.submitted.take();
            self.submitted_ack = None;
            if reference_matches
                && submitted.as_deref() == Some(self.input.read(cx).value().as_ref())
            {
                self.input
                    .update(cx, |input, cx| input.set_value("", window, cx));
            }
            if reference_matches && self.images == self.submitted_images {
                self.images.clear();
            }
        }
        if self
            .submitted_ack
            .is_some_and(|id| state.rejected_submission == Some(id))
            || state.status == codex::Status::Disconnected
        {
            self.submitted = None;
            self.submitted_ack = None;
        }
        let optimistic_items: Vec<_> = self
            .bubble_entrances
            .iter()
            .filter_map(|pending| pending.optimistic.clone())
            .filter(|item| {
                !state
                    .saved
                    .items
                    .iter()
                    .any(|saved| saved["id"] == item["id"])
            })
            .collect();
        let transcript = match &self.transcript_cache {
            Some(cache) if cache.matches(&state, &optimistic_items) => cache.clone(),
            _ => {
                let cache = Rc::new(TranscriptRenderCache::new(state, optimistic_items));
                self.transcript_cache = Some(cache.clone());
                cache
            }
        };
        let state = transcript.state.clone();
        if transcript.has_timestamp && !self.timestamp_tick_pending.replace(true) {
            let pending = self.timestamp_tick_pending.clone();
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(30))
                    .await;
                pending.set(false);
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }

        // Tool output/status and same-length streaming replacements also change layout.
        let content = Some(transcript.content);
        let near_bottom = self.scroll.max_offset().y + self.scroll.offset().y
            <= rpx(SPACE_20).to_pixels(window.rem_size());
        if let Some(toward_bottom) = self.pending_composer_scroll.take() {
            if self.preview_child.is_some() {
                self.preview_follow_latest = toward_bottom
                    && self.scroll.max_offset().y + self.scroll.offset().y <= gpui::px(1.);
            } else {
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
        }
        // A blocking edit confirmation owns the visible source. Session updates
        // must not restart bottom-follow and pull that source out from its tray.
        if self.edit_confirmation.is_some() {
            self.smooth_follow = false;
            self.follow_after_submit = false;
        }
        if self.follow_after_submit {
            self.composer_pin_bottom = true;
            self.set_composer_collapsed(false, cx);
        }
        let changed_thread = self.stream_thread != state.saved.thread_id;
        if changed_thread {
            self.selection_views.clear();
            self.selection_dismissed = None;
            if self
                .reference
                .as_ref()
                .is_some_and(|quote| quote.thread_id != state.saved.thread_id)
            {
                self.reference.clone_from(&state.saved.draft_reference);
                self.reference_expanded = false;
            }
            self.stream_text.clear();
            self.stream_thread.clone_from(&state.saved.thread_id);
        }
        if cx.reduce_motion() {
            // A later preference toggle must not resurrect an older partial prefix.
            for (text, shown) in self.stream_text.values_mut() {
                *shown = text.len();
            }
        }
        let initial_transcript = self.last_content.is_none() || changed_thread;
        self.stream_text
            .retain(|key, _| transcript.stream_keys.contains(key));
        if self.follow_after_submit {
            self.last_follow_offset = None;
        }
        let now = cx.background_executor().now();
        let elapsed = self
            .motion_frame
            .replace(now)
            .map_or(0.0, |previous| now.duration_since(previous).as_secs_f32());
        if self.preview_child.is_none()
            && self.edit_confirmation.is_none()
            && (self.follow_after_submit
                || (content != self.last_content
                    && (near_bottom || self.smooth_follow)
                    && !self.composer_collapsed))
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
                    window.on_next_frame(move |_, cx| {
                        pending.set(false);
                        let _ = owner.update(cx, |_, cx| cx.notify());
                    });
                }
            }
        }
        self.last_content = content;
        let task_key = transcript.task_key;
        let task_cache = match &self.task_cache {
            Some(cache) if cache.key == task_key => cache.clone(),
            _ => {
                let cache = Rc::new(task_cache(&state.saved.items, task_key));
                self.task_cache = Some(cache.clone());
                cache
            }
        };
        // Codex keeps its empty bootstrap interstitial. Claude starts in the composer.
        // Existing work,
        // actionable requests, and failures keep their transcript and recovery UI.
        if state.saved.provider != codex::Provider::Claude
            && state.status == codex::Status::Starting
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
        let response_groups = &transcript.response_groups;
        let response_ends = &transcript.response_ends;
        let response_prose = &transcript.response_prose;
        let minimap_entries = &transcript.minimap_entries;
        let mut minimap_anchors = Vec::new();
        // The first rendered child at or after each item, for jump-to-start.
        let mut item_children = Vec::with_capacity(state.saved.items.len());
        let mut message_child = 0;
        let mut rendered_selection_keys = HashSet::new();
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
            && (!state.status.busy()
                || (state.status == codex::Status::Starting
                    && state.saved.provider == codex::Provider::Claude));
        for (index, item) in state
            .saved
            .items
            .iter()
            .chain(std::iter::once(&serde_json::Value::Null))
            .enumerate()
        {
            // Flush before inspecting the next source item: the previous final item
            // may be hidden commentary, a checklist, or a collapsed tool.
            if let Some(group_index) = response_ends[index] {
                messages =
                    messages.child(self.response_footer(&response_groups[group_index], window, cx));
                message_child += 1;
            }
            if index < state.saved.items.len() {
                item_children.push(message_child);
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
                                .h_auto()
                                .min_h(rpx(CHROME_CONTROL_H))
                                .flex_wrap()
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
                if expanded && !work_has_parent_activity(&state, index) {
                    message_child += 1;
                    messages = messages.child(
                        div()
                            .id(("codex-work-empty", index))
                            .debug_selector(move || format!("codex-work-empty-{index}"))
                            .w_full()
                            .max_w(rpx(CHAT_MAX_W))
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(if active {
                                "No parent activity has been reported yet."
                            } else {
                                "No parent activity was reported."
                            }),
                    );
                }
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
            if kind == "subAgentActivity" {
                continue;
            }
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
            if kind == "providerNotice" {
                message_child += 1;
                messages = messages.child(
                    div()
                        .id(("provider-notice", index))
                        .debug_selector(move || format!("provider-notice-{index}"))
                        .role(gpui::Role::Status)
                        .w_full()
                        .max_w(rpx(CHAT_MAX_W))
                        .min_w_0()
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .gap(rpx(SPACE_SM))
                        .py(rpx(SPACE_LG))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_LG))
                                .child(crate::icons::icon("message-warning", ICON_MD, c::AMBER()))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_size(rpx(TEXT_SMALL))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .child(
                                            item["title"]
                                                .as_str()
                                                .unwrap_or("Provider warning")
                                                .to_owned(),
                                        ),
                                ),
                        )
                        .child(div().w_full().min_w_0().pl(rpx(ICON_MD + SPACE_LG)).child(
                            literal_notice_text(
                                gpui::SharedString::from(format!("provider-notice-text-{index}")),
                                item["text"].as_str().unwrap_or_default(),
                            ),
                        )),
                );
                continue;
            }
            if tasks::is_checklist(item) {
                // Checklist updates belong in the docked panel, not the transcript.
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
                        self.expanded.contains(&work_key_at(&state, item_index))
                            || (tool_visible(&state, item_index)
                                && work_is_active_at(&state, item_index))
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
                        "Running commands…"
                    } else {
                        "Ran commands"
                    }
                } else if running {
                    "Working…"
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
                        let failed = tool_failed(tool);
                        let activity =
                            tool_activity_summary(tool, tool_is_running(&state, detail_index));
                        let mut label_run = window.text_style().to_run(activity.len());
                        label_run.font.family = crate::fonts::UI_FAMILY.into();
                        let label_width = truncation_safe_label_width(
                            &activity,
                            rpx(TEXT_SMALL).to_pixels(window.rem_size()),
                            &label_run,
                            window.text_system(),
                        );
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
                                            if failed {
                                                "message-error"
                                            } else if tool["type"] == "reasoning" {
                                                "brain"
                                            } else {
                                                "terminal"
                                            },
                                            ICON_SM,
                                            if failed { c::FORM_ERROR() } else { c::FG_DIM() },
                                        )),
                                )
                                .child(
                                    div()
                                        .id(("codex-tool-row-label", detail_index))
                                        .debug_selector(move || {
                                            format!("codex-tool-row-label-{detail_index}")
                                        })
                                        .w(label_width)
                                        .min_w_0()
                                        .truncate()
                                        .text_size(rpx(TEXT_SMALL))
                                        .text_color(c::FG_DIM())
                                        .child(shimmer_label(
                                            ("codex-tool-detail-shimmer", detail_index).into(),
                                            activity,
                                            work_is_active_at(&state, detail_index)
                                                && tool_visible(&state, detail_index),
                                            cx,
                                        )),
                                )
                                .aria_expanded(tool_expanded)
                                .child(
                                    crate::icons::icon(
                                        if tool_expanded {
                                            "chev-down"
                                        } else {
                                            "chev-right"
                                        },
                                        ICON_SM,
                                        c::FG_DIM(),
                                    )
                                    .flex_shrink_0(),
                                )
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
                                    .child(copy_control("codex-copy-tool", "Copy tool call", copy))
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
            let plan_details = kind == "plan"
                && state.saved.provider == codex::Provider::Claude
                && item["tool"].is_string();
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
            let copy = if kind == "userMessage" {
                reference_prompt_copy(&text)
            } else {
                text.clone()
            };
            let text = if kind == "userMessage" {
                reference_prompt_markdown(&text)
            } else {
                text
            };
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
                        window.on_next_frame(move |_, cx| {
                            pending.set(false);
                            let _ = owner.update(cx, |_, cx| cx.notify());
                        });
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
                                .child(
                                    if kind == "userMessage"
                                        && self.bubble_entrances.iter().any(|pending| {
                                            pending.optimistic.as_ref().is_some_and(|bubble| {
                                                bubble["id"] == item["id"]
                                                    && bubble["localSteering"] == true
                                            })
                                        })
                                    {
                                        "You · Steering…".to_owned()
                                    } else if tool {
                                        tool_summary(item)
                                    } else {
                                        role.to_owned()
                                    },
                                ),
                        )
                        .when(tool || plan_details, |row| {
                            let keyboard_toggle = toggle.clone();
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
                                .debug_selector(move || format!("codex-toggle-details-{index}"))
                                .aria_expanded(expanded)
                                .tab_index(0)
                                .focus_visible(|style| style.bg(c::BG_HOVER()))
                                .on_key_down(cx.listener(
                                    move |this, event: &gpui::KeyDownEvent, _, cx| {
                                        if action_key(event) {
                                            this.toggle_expansion(&keyboard_toggle, cx);
                                            cx.stop_propagation();
                                        }
                                    },
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
                        .when(kind != "userMessage", |row| {
                            row.child(copy_control("codex-copy", "Copy message", copy.clone()))
                        }),
                );
            }
            if !tool {
                let html_message = item["id"]
                    .as_str()
                    .map_or_else(|| format!("message-{index}"), str::to_owned);
                let html_extensions = self
                    .html
                    .update(cx, |html, cx| html.extensions(&html_message, cx));
                if let Some(editor) = self.message_editor.as_ref().filter(|editor| {
                    editor.index == index && editor.message_id == item["id"].as_str().unwrap_or("")
                }) {
                    row = row.child(self.render_inline_editor(editor, window, cx));
                } else {
                    row = row.child(
                        div()
                            .id(format!("codex-message-body-{index}"))
                            .debug_selector(move || format!("codex-message-body-{index}"))
                            .child({
                                let owner_key = format!(
                                    "{}:{}",
                                    state.saved.thread_id.as_deref().unwrap_or(""),
                                    html_message
                                );
                                rendered_selection_keys.insert(owner_key.clone());
                                let owner = if let Some(owner) =
                                    self.selection_views.get(&owner_key).filter(|owner| {
                                        text.starts_with(owner.read(cx).source().as_ref())
                                    }) {
                                    owner.clone()
                                } else {
                                    let owner = cx.new(|cx| TextViewState::markdown(&text, cx));
                                    self.subscriptions
                                        .push(cx.observe(&owner, |_, _, cx| cx.notify()));
                                    self.selection_views.insert(owner_key.clone(), owner.clone());
                                    owner
                                };
                                owner.update(cx, |owner, cx| owner.set_text_incremental(&text, cx));
                                let view = TextView::new(&owner)
                                    .markdown_extensions(html_extensions)
                                    .style(markdown_style(window))
                                    .selectable(true);
                                if kind == "agentMessage" {
                                    let owner = cx.entity().downgrade();
                                    let source = text.clone();
                                    let runtime = self.script_runtime.clone();
                                    let cwd = state.saved.cwd.clone();
                                    let finalized = (item["groveCompleted"] == true || item["status"] == "completed") && item["status"] != "inProgress" && self.preview_child.is_none();
                                    view.code_block_actions(move |block, window, cx| {
                                        let code = block.code().to_string();
                                        let key = format!("{owner_key}:code:{}", block.span.as_ref().map_or(0, |span| span.start));
                                        let focus = owner.update(cx, |this, cx| this.message_focus.entry(key.clone()).or_insert_with(|| cx.focus_handle()).clone()).ok();
                                        let copied = owner.upgrade().is_some_and(|owner| owner.read(cx).copied_code_blocks.contains_key(&key));
                                        let label = if copied { "Copied" } else { "Copy code" };
                                        let keyboard_owner = owner.clone();
                                        let keyboard_key = key.clone();
                                        let keyboard_code = code.clone();
                                        let click_owner = owner.clone();
                                        let click_code = code.clone();
                                        let copy = header_control("codex-copy-code", label)
                                            .when_some(focus, |this, focus| this.track_focus(&focus))
                                            .debug_selector(move || if copied { "codex-code-copied".into() } else { "codex-copy-code".into() })
                                            .tab_index(0)
                                            .focus_visible(|style| style.bg(c::BG_HOVER()))
                                            .child(crate::icons::icon(if copied { "check" } else { "copy" }, ICON_MD, c::FG_DIM()))
                                            .on_key_down(move |event, _, cx| {
                                                if action_key(event) {
                                                    let _ = keyboard_owner.update(cx, |this, cx| this.copy_code_block(keyboard_key.clone(), keyboard_code.clone(), cx));
                                                    cx.stop_propagation();
                                                }
                                            })
                                            .on_click(move |_, _, cx| {
                                                let _ = click_owner.update(cx, |this, cx| this.copy_code_block(key.clone(), click_code.clone(), cx));
                                            });
                                        let mut actions = div().flex().gap(rpx(SPACE_SM)).child(copy);
                                        let language = block.lang().map(|s| s.to_string()).unwrap_or_default();
                                        let closed = block.span.as_ref().and_then(|span| source.get(span.start..span.end)).is_some_and(closed_shell_fence);
                                        if finalized && closed && !code.trim().is_empty() && snippet_shell_supported(&language) {
                                            if let Some(runtime) = runtime.clone() {
                                                let event_owner = owner.clone();
                                                let cwd = cwd.clone();
                                                let owner = grove_core::session_meta::project_owner(&cx.global::<crate::settings::SettingsState>().store.projects, &cwd).map(|p| p.path.clone());
                                                let available = owner.is_ok();
                                                actions = actions.child(transcript_action("codex-run-code", if available { "Run" } else { "Checkout unavailable" }, false, window, cx)
                                                    .disabled(!available)
                                                    .debug_selector(|| "codex-run-code".into())
                                                    .icon(gpui_component::Icon::default().path("icons/play.svg").size(rpx(ICON_SM)))
                                                    .on_click(move |_, _, cx| {
                                                        let owner = grove_core::session_meta::project_owner(&cx.global::<crate::settings::SettingsState>().store.projects, &cwd).map(|p| p.path.clone());
                                                        if let Ok(project) = owner {
                                                            let launched = runtime.update(cx, |runtime, cx| runtime.spawn_checkout_command(&project, &cwd, &code, cx));
                                                            if launched { let _ = event_owner.update(cx, |_,cx| cx.emit(CodexViewEvent::ScriptLaunched)); }
                                                        }
                                                    }));
                                            } else {
                                                actions = actions.child(transcript_action("codex-run-code", "Checkout unavailable", false, window, cx).disabled(true).debug_selector(|| "codex-run-code".into()).icon(gpui_component::Icon::default().path("icons/play.svg").size(rpx(ICON_SM))));
                                            }
                                        }
                                        actions
                                    })
                                } else {
                                    view
                                }
                            }),
                    );
                }
                let html_entity = self.html.clone();
                let linked =
                    html_entity.update(cx, |html, cx| html.linked(&html_message, window, cx));
                row = row.children(linked);
                if kind == "agentMessage" {
                    let plans: Vec<_> = self
                        .session
                        .read(cx)
                        .state
                        .saved
                        .rich_plans
                        .revisions
                        .iter()
                        .filter(|plan| plan.message_id == html_message)
                        .map(|plan| {
                            (
                                plan.document.plan_id.clone(),
                                plan.document.revision_id.clone(),
                            )
                        })
                        .collect();
                    for (plan_id, revision_id) in plans {
                        let key = (plan_id, revision_id);
                        row = row.child(super::native_plan::inline(&self.native_plans[&key], cx));
                    }
                    let errors = state.saved.rich_plans.errors.clone();
                    for error in errors
                        .into_iter()
                        .filter(|error| error.message_id == html_message)
                    {
                        if let Some(limit) = &error.size_limit {
                            let recovery_roots: Vec<std::path::PathBuf> = state
                                .saved
                                .roots
                                .iter()
                                .map(|root| root.wt_path.clone().into())
                                .collect();
                            let path = super::inline_html::recovery_path(
                                &limit.path,
                                std::path::Path::new(&state.saved.cwd),
                                &recovery_roots,
                            );
                            let recovery_cwd = std::path::PathBuf::from(&state.saved.cwd);
                            let actions = path
                                .into_iter()
                                .map(|path| {
                                    let recovery_cwd = recovery_cwd.clone();
                                    let recovery_roots = recovery_roots.clone();
                                    transcript_action(
                                        format!("reveal-oversized-plan-{}", error.message_id),
                                        "Reveal file",
                                        false,
                                        window,
                                        cx,
                                    )
                                    .icon(
                                        gpui_component::Icon::default()
                                            .path("icons/folder.svg")
                                            .size(rpx(ICON_SM)),
                                    )
                                    .on_click(move |_, _, cx| {
                                        if let Some(path) = super::inline_html::recovery_path(
                                            &path,
                                            &recovery_cwd,
                                            &recovery_roots,
                                        ) {
                                            cx.reveal_path(&path);
                                        }
                                    })
                                    .into_any_element()
                                })
                                .collect();
                            let is_plan = super::inline_html::recovery_path(
                                std::path::Path::new(&error.source),
                                std::path::Path::new(&state.saved.cwd),
                                &recovery_roots,
                            )
                            .as_ref()
                                == Some(&limit.path);
                            row = row.child(super::inline_html::oversized_card(
                                if is_plan { "Plan too large" } else { "Plan asset too large" },
                                limit,
                                if is_plan { "Move embedded images into separate assets and reference their paths." } else { "Reduce this asset’s size, then link a new plan revision." },
                                actions, cx,
                            ));
                            continue;
                        }
                        let message_id = error.message_id.clone();
                        let source = error.source.clone();
                        row =
                            row.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(rpx(SPACE_LG))
                                    .p(rpx(SPACE_2XL))
                                    .child(div().text_color(c::FORM_ERROR()).child(format!(
                                        "Couldn’t open native plan: {}",
                                        error.error
                                    )))
                                    .child(div().text_color(c::FG_DIM()).child(error.source))
                                    .child(
                                        transcript_action(
                                            format!("retry-native-plan-{message_id}"),
                                            "Retry plan import",
                                            false,
                                            window,
                                            cx,
                                        )
                                        .icon(
                                            gpui_component::Icon::default()
                                                .path("icons/restart.svg")
                                                .size(rpx(ICON_SM)),
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.session.update(cx, |session, cx| {
                                                    session.send(
                                                        Action::RetryPlanImport {
                                                            message_id: message_id.clone(),
                                                            source: source.clone(),
                                                        },
                                                        cx,
                                                    );
                                                });
                                            }),
                                        ),
                                    ),
                            );
                    }
                }

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
            if plan_details && expanded {
                row = row.child(
                    div()
                        .id(("codex-plan-detail", index))
                        .debug_selector(move || format!("codex-plan-detail-{index}"))
                        .child(
                            TextView::markdown(
                                ("codex-plan-tool-text", index),
                                format!(
                                    "```text\n{}\n```",
                                    tool_detail(item).replace("```", "` ` `")
                                ),
                            )
                            .style(markdown_style(window))
                            .selectable(true),
                        ),
                );
            }
            if kind == "agentMessage" {
                if let Some(group_index) = response_prose[index] {
                    let group = &response_groups[group_index];
                    let key = group.key.clone();
                    let source_key = format!("{}:source:{id}", group.key);
                    if !self.message_focus.contains_key(&source_key) {
                        let focus = cx.focus_handle();
                        self.subscriptions
                            .push(cx.on_focus_in(&focus, window, |_, _, cx| cx.notify()));
                        self.subscriptions
                            .push(cx.on_focus_out(&focus, window, |_, _, _, cx| cx.notify()));
                        self.message_focus.insert(source_key.clone(), focus);
                    }
                    let focus = self.message_focus[&source_key].clone();
                    row = row.track_focus(&focus).tab_index(0).on_hover(cx.listener(
                        move |this, hovered: &bool, _, cx| {
                            if *hovered {
                                this.message_hover.insert(key.clone());
                            } else {
                                this.message_hover.remove(&key);
                            }
                            cx.notify();
                        },
                    ));
                }
            }
            if kind == "userMessage" {
                row = div()
                    .id(("codex-message-block", index))
                    .flex()
                    .flex_col()
                    .w_full()
                    .max_w(rpx(USER_BUBBLE_MAX_W))
                    .gap(rpx(SPACE_SM))
                    .child(row);
                let key = format!(
                    "{}:{id}",
                    state.saved.thread_id.as_deref().unwrap_or_default()
                );
                if !self.message_focus.contains_key(&key) {
                    let focus = cx.focus_handle();
                    self.subscriptions
                        .push(cx.on_focus_in(&focus, window, |_, _, cx| cx.notify()));
                    self.subscriptions
                        .push(cx.on_focus_out(&focus, window, |_, _, _, cx| cx.notify()));
                    self.message_focus.insert(key.clone(), focus);
                }
                let focus = self.message_focus[&key].clone();
                let visible =
                    self.message_hover.contains(&key) || focus.contains_focused(window, cx);
                let copied = self
                    .copied_message
                    .as_ref()
                    .is_some_and(|(message, _)| message == &key);
                let hover_key = key.clone();
                row = row.track_focus(&focus).tab_index(0).on_hover(cx.listener(
                    move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.message_hover.insert(hover_key.clone());
                        } else {
                            this.message_hover.remove(&hover_key);
                        }
                        cx.notify();
                    },
                ));
                let mut actions = div()
                    .id(("message-actions", index))
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_MD))
                    .h(rpx(CHROME_CONTROL_H))
                    .justify_end();
                if let Some(timestamp) = item["groveReceivedAt"].as_u64() {
                    actions = actions.child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_MUTE())
                            .child(relative_message_time(timestamp)),
                    );
                }
                let unavailable = state.editable_message(index).err();
                if self.preview_child.is_none() {
                    actions = actions.child(
                        header_control("codex-edit-message", "Edit sent message")
                            .aria_description(
                                unavailable
                                    .unwrap_or_else(|| "Edit message and restore workspace".into()),
                            )
                            .debug_selector(move || format!("codex-edit-message-{index}"))
                            .tab_index(0)
                            .focus_visible(|style| style.bg(c::BG_HOVER()))
                            .child(crate::icons::icon("edit", ICON_MD, c::FG_DIM()))
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, window, cx| {
                                    if action_key(event) {
                                        this.request_edit(index, window, cx);
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.request_edit(index, window, cx);
                            })),
                    );
                }
                let keyboard_key = key.clone();
                let keyboard_text = copy.clone();
                let icon =
                    crate::icons::icon(if copied { "check" } else { "copy" }, ICON_MD, c::FG_DIM());
                let mut copy_button = if copied {
                    header_control_frame("codex-message-copy", "Copied", c::BG_HOVER())
                } else {
                    header_control("codex-message-copy", "Copy message")
                }
                .relative()
                .debug_selector(|| "codex-copy".into())
                .tab_index(0)
                .focus_visible(|style| style.bg(c::BG_HOVER()))
                .child(if copied {
                    super::motion::fast(icon, ("message-copy-check", index), cx)
                } else {
                    icon.into_any_element()
                })
                .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                    if action_key(event) {
                        this.copy_message(keyboard_key.clone(), keyboard_text.clone(), cx);
                        cx.stop_propagation();
                    }
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.copy_message(key.clone(), copy.clone(), cx);
                }));
                if copied {
                    copy_button = copy_button.child(
                        div()
                            .absolute()
                            .bottom(rpx(CHROME_CONTROL_H + SPACE_SM))
                            .right_0()
                            .child(tooltip("Copied", window).build(window, cx)),
                    );
                }
                actions = actions.child(copy_button);
                // Keep the footer geometry and keyboard targets mounted while hidden.
                row = row.child(if visible || copied {
                    super::motion::fast(actions, ("message-actions-show", index), cx)
                } else {
                    actions.opacity(0.0).into_any_element()
                });
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
                    // Only assistant prose gives back the old per-item footer gap;
                    // tool groups and user-message boundaries retain their spacing.
                    .when(kind == "agentMessage", |anchor| anchor.mb(rpx(-SPACE_LG)))
                    .child(row),
            );
        }
        if self.preview_child.is_some() {
            // Stay pinned through layout growth until a wheel gesture starts browsing.
            // Returning to the bottom resumes following without affecting the composer.
            if self.preview_follow_latest {
                self.scroll.scroll_to_bottom();
            }
            return div()
                .size_full()
                .min_h_0()
                .min_w_0()
                .flex()
                .flex_col()
                .font_family(crate::fonts::UI_FAMILY)
                .text_color(c::FG())
                .child(messages.px(rpx(SPACE_SM)))
                .into_any_element();
        }
        if let Some(editor) = self
            .message_editor
            .as_ref()
            .filter(|editor| editor.restored)
        {
            messages = messages.child(
                div()
                    .w_full()
                    .max_w(rpx(CHAT_MAX_W))
                    .child(self.render_inline_editor(editor, window, cx)),
            );
        }
        messages =
            messages.children(self.question_blocks_at(state.saved.items.len(), &state, window, cx));
        // Hidden, replaced and removed message owners must be dropped so the
        // native selection controller cannot copy their last painted text.
        self.selection_views
            .retain(|key, _| rendered_selection_keys.contains(key));
        let minimap = self.render_message_minimap(minimap_entries, &minimap_anchors, window, cx);
        let selection_overlay = self.render_selection_reply(&state, window, cx);
        let transcript_bounds = self.transcript_bounds.clone();
        let bounds_owner = cx.entity().downgrade();
        let transcript = div()
            .id("codex-transcript")
            .relative()
            .child(
                gpui::canvas(
                    move |bounds, _, cx| {
                        if transcript_bounds.replace(bounds) != bounds {
                            let _ = bounds_owner.update(cx, |_, cx| cx.notify());
                        }
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.notify()),
            )
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.selection_dismissed = window.text_selection_snapshot(cx);
                    cx.notify();
                }
            }))
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .when(!minimap_entries.is_empty(), |transcript| {
                transcript.pl(rpx(CHROME_CONTROL_H))
            })
            .child(messages)
            .children(selection_overlay);
        let mut surface = div()
            .id("codex-conversation")
            .relative()
            .w_full()
            .when(self.dashboard_tray.is_none(), gpui::Styled::h_full)
            .min_h_0()
            .flex()
            .flex_col()
            .font_family(crate::fonts::UI_FAMILY)
            .text_size(rpx(TEXT_BODY))
            .text_color(c::FG())
            .when(!focused_launch, |surface| {
                surface
                    // Anchor navigation to the pane, independent of the transcript's
                    // changing height above tasks, attachments, and the composer.
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .w(rpx(CHROME_CONTROL_H))
                            .flex()
                            .child(minimap),
                    )
                    .child(transcript)
            })
            .when(
                focused_launch && self.dashboard_tray.is_none(),
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
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && this.model_from_card {
                    this.model_from_card = false;
                    this.open_menu(ComposerMenu::Settings, window, cx);
                    this.card_model_focus.focus(window, cx);
                    cx.stop_propagation();
                } else if event.keystroke.key == "escape" && this.menu.is_some() {
                    let index = if this.menu == Some(ComposerMenu::Access) {
                        2
                    } else {
                        0
                    };
                    this.close_menu(cx);
                    this.option_focus[index].focus(window, cx);
                    cx.stop_propagation();
                }
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && this.menu == Some(ComposerMenu::Settings) {
                    this.close_menu(cx);
                    this.option_focus[0].focus(window, cx);
                    cx.stop_propagation();
                    return;
                }
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
        if let Some(activity) = live_activity(&state)
            .filter(|_| state.approvals.is_empty() && state.elicitations.is_empty())
        {
            let running = compacting(&state)
                || (state.status.busy()
                    && state.questions.is_empty()
                    && state.elicitations.is_empty()
                    && state.approvals.is_empty());
            surface = surface.child(
                div()
                    .id("codex-live-activity")
                    .role(gpui::Role::Status)
                    .debug_selector(|| "codex-live-activity".into())
                    .w_full()
                    .flex_shrink_0()
                    .flex()
                    .when(!minimap_entries.is_empty(), |row| {
                        row.pl(rpx(CHROME_CONTROL_H))
                    })
                    .py(rpx(SPACE_SM))
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .justify_center()
                            .px(rpx(SPACE_20))
                            .child(
                                div()
                                    .w_full()
                                    .max_w(rpx(CHAT_MAX_W))
                                    .truncate()
                                    .id("codex-live-activity-text")
                                    .debug_selector(|| "codex-live-activity-text".into())
                                    .child(shimmer_label(
                                        "codex-activity-shimmer".into(),
                                        activity,
                                        running,
                                        cx,
                                    )),
                            ),
                    ),
            );
        }
        let agent_controls = if !state.approvals.is_empty() || !state.elicitations.is_empty() {
            Some(
                div()
                    .id("agent-controls")
                    .debug_selector(|| "agent-controls".into())
                    .min_w_0()
                    .min_h_0()
                    .max_h(
                        rpx(QUESTION_PANEL_MAX_H)
                            .to_pixels(window.rem_size())
                            .min(window.viewport_size().height * APPROVAL_TRAY_VIEWPORT_SHARE),
                    )
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_LG))
                    .mx(rpx(composer_tray_inset(window)))
                    .when(!state.approvals.is_empty(), |tray| {
                        tray.child(self.approval_panel(&state, window, cx))
                    })
                    .when(!state.elicitations.is_empty(), |tray| {
                        tray.child(self.elicitation_panel(&state, window, cx))
                    }),
            )
        } else {
            None
        };
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
        let task_strip = task_cache.list.as_ref().map(|list| {
            let rendered = message_child;
            let jumps = list
                .tasks
                .iter()
                .map(|task| {
                    item_children
                        .get(task.started_at)
                        .copied()
                        .unwrap_or(rendered)
                        .min(rendered.saturating_sub(1))
                })
                .collect();
            self.task_strip(list, state.status, jumps, window, cx)
        });
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
        let draft_disabled = self.message_editor.is_some()
            || self.edit_confirmation.is_some()
            || self.edit_request_pending
            || state.saved.pending_edit.is_some()
            || state.status == codex::Status::Disconnected
            || (state.status == codex::Status::Starting
                && state.saved.provider != codex::Provider::Claude)
            || self.submitted.is_some()
            || self.pending_image_pastes > 0
            || stopping
            || (self.input.read(cx).value().trim().is_empty()
                && self.images.is_empty()
                && self.reference.is_none());
        let steer_available = active_turn_redirect_available(&state);
        let editing = self
            .queue_editor
            .as_ref()
            .is_some_and(|editor| editor.loaded);
        let primary_disabled = if editing {
            self.queue_editor
                .as_ref()
                .is_some_and(|editor| editor.saving)
                || (self.input.read(cx).value().trim().is_empty()
                    && self.images.is_empty()
                    && self
                        .queue_editor
                        .as_ref()
                        .is_none_or(|editor| editor.reference.is_none()))
        } else {
            draft_disabled
        };
        let primary_label = if editing {
            "Save queued message · Enter · Shift+Enter for a new line"
        } else if busy {
            "Queue · Enter · Send after the current turn finishes · Shift+Enter for a new line"
        } else {
            "Send · Enter · Start a new turn · Shift+Enter for a new line"
        };
        // An absent menu must not leave an empty flex child and an extra rail gap.
        let choices = (!self.composer_collapsed && self.suggestion_choices(&state, cx).is_some())
            .then(|| self.composer_choices(&state, window, cx));
        let collapsed = self.composer_collapsed;
        let composer_min_h = if self.dashboard_tray.is_some() {
            DASHBOARD_COMPOSER_MIN_H
        } else {
            COMPOSER_MIN_H
        };
        let target = self.composer_body_target.clone();
        let expanded_height = self.composer_expanded_height.clone();
        if self.composer_transition_from.is_some() {
            target.set(if collapsed {
                rpx(CHROME_CONTROL_H).to_pixels(window.rem_size())
            } else {
                expanded_height
                    .get()
                    .max(rpx(composer_min_h).to_pixels(window.rem_size()))
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
                                composer_min_h
                            }))
                            .capture_any_mouse_down(cx.listener(
                                |this, _: &gpui::MouseDownEvent, window, cx| {
                                    crate::platform::inline_html::NativeDocument::focus_host(
                                        window,
                                    );
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
                                        .cursor_pointer()
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
        let mut surface =
            surface.child(
                div()
                    // May shrink below its content so a short pane squeezes
                    // the task strip instead of clipping the composer.
                    .min_h_0()
                    .w_full()
                    .when(focused_launch && self.dashboard_tray.is_none(), |rail| rail.my_auto().pt(rpx(SPACE_3XL)))
                    .px(rpx(SPACE_20))
                    // Match the transcript's reserved navigation gutter so both
                    // centered columns retain the same edges at every pane width.
                    .when(!minimap_entries.is_empty(), |rail| {
                        rail.pl(rpx(SPACE_20 + CHROME_CONTROL_H))
                    })
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
                            .when(focused_launch && self.dashboard_tray.is_none(), |rail| rail.child(self.launch_intro(&state)))
                            .when_some(self.session_notice(&state, window, cx), gpui::ParentElement::child)
                            .when(self.dashboard_tray.is_some(), |rail| {
                                let session = self.session.read(cx);
                                if session.catalog_loading {
                                    rail.child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child(format!("Loading {} options…", state.saved.provider.label())))
                                } else if let Some(error) = &session.catalog_error {
                                    rail.child(div().flex().flex_col().gap(rpx(SPACE_SM))
                                        .child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FORM_ERROR()).child(error.clone()))
                                        .child(header_control("dashboard-catalog-retry", "Retry provider options").track_focus(&self.dashboard_retry_focus).focus_visible(|s|s.bg(c::BG_HOVER())).w_auto().tab_index(0).gap(rpx(SPACE_SM))
                                            .child(crate::icons::icon("restart",ICON_SM,c::FG_DIM())).child("Retry")
                                            .on_click(cx.listener(|this,_,_,cx|this.retry_dashboard_catalog(cx)))
                                            .on_key_down(cx.listener(|this,event:&gpui::KeyDownEvent,_,cx|{if action_key(event){this.retry_dashboard_catalog(cx);cx.stop_propagation();}}))))
                                } else {rail}
                            })
                            .when_some(agent_controls, gpui::ParentElement::child)
                            .when_some(task_strip, gpui::ParentElement::child)
                            .when_some(choices, gpui::ParentElement::child)
                            .when(!self.queue_motion_rows.is_empty(), |rail| rail.child(queued))
                            .child(div().w_full().flex().flex_col().flex_shrink_0()
                            .children(self.render_reference_preview(window, cx))
                            .child(
                        div()
                            .id("codex-composer")
                            .debug_selector(|| "codex-composer".into())
                            .flex_shrink_0()
                            .w_full()
                            .max_w(rpx(CHAT_MAX_W))
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_LG))
                            .px(rpx(SPACE_2XL))
                            .pt(rpx(SPACE_3XL))
                            .pb(rpx(SPACE_2XL))
                            .rounded(rpx(RADIUS_PANEL))
                            .when(self.queue_editor.as_ref().map_or_else(|| self.reference.is_some(), |editor| editor.reference.is_some()), |composer| composer.rounded_none().rounded_b(rpx(RADIUS_PANEL)))
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
                                    .children(self.option_triggers(&state, window, cx))
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
                                            .when(busy, |row| row.child(
                                                composer_icon("codex-stop", if stopping { "Stopping the current turn" } else { "Stop · Interrupt the current turn" }, "stop", stopping || state.turn_id.is_none(), true)
                                                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                                                        if action_key(event) { this.interrupt(cx); cx.stop_propagation(); }
                                                    }))
                                                    .on_click(cx.listener(|this, _, _, cx| this.interrupt(cx)))
                                            ))
                                            .when(!busy, |row| row.child(
                                                composer_icon("codex-send", primary_label, "arrow-up", primary_disabled, true)
                                                    .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                                                        if action_key(event) {
                                                            if !primary_disabled { this.submit(window, cx); }
                                                            cx.stop_propagation();
                                                        }
                                                    }))
                                                    .on_click(cx.listener(move |this, _, window, cx| {
                                                        if !primary_disabled { this.submit(window, cx); }
                                                    }))
                                            ))
                                    ),
                            )
                            ))
                            .children(self.dashboard_tray.clone())
                            .child(
                                div()
                                    .id("codex-composer-hints")
                                    .debug_selector(|| "codex-composer-hints".into())
                                    .aria_description(STEER_DESCRIPTION)
                                    .w_full()
                                    .px(rpx(SPACE_2XL))
                                    .flex()
                                    .flex_wrap()
                                    .justify_end()
                                    .text_right()
                                    .whitespace_normal()
                                    .gap(rpx(SPACE_SM))
                                    .text_size(rpx(TEXT_MICRO))
                                    .text_color(c::FG_DIM())
                                    .child(if stopping {
                                        "Stopping…"
                                    } else if self.submitted.is_some() {
                                        "Sending…"
                                    } else if editing {
                                        "Enter to save"
                                    } else if busy && steer_available {
                                        "Enter to queue · ⌘Enter to steer"
                                    } else if busy {
                                        "Enter to queue"
                                    } else {
                                        "Enter to send"
                                    })
                            )
                            .when(focused_launch && self.dashboard_tray.is_none(), |rail| rail.child(self.launch_starters(provider, cx))),
                    ),
            );
        if let Some(menu) = self.menu {
            let popup = if menu == ComposerMenu::Settings {
                Some(self.model_effort_card(&state, window, cx))
            } else if menu == ComposerMenu::Model {
                self.model_picker.clone().map(IntoElement::into_any_element)
            } else {
                self.popup.clone().map(IntoElement::into_any_element)
            };
            if let Some(popup) = popup {
                let index = if menu == ComposerMenu::Access { 2 } else { 0 };
                let bounds = self.option_bounds[index].get();
                let gap = rpx(SPACE_LG).to_pixels(window.rem_size());
                return surface
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
                    .into_any_element();
            }
        }
        if self.edit_confirmation.is_some() {
            surface = surface.child(self.render_edit_confirmation(window, cx));
        }
        surface.into_any_element()
    }
}
/// Paint a visual source aperture without creating any pointer hit-test hole.
fn paint_confirmation_veil(
    pane: gpui::Bounds<gpui::Pixels>,
    source: gpui::Bounds<gpui::Pixels>,
    window: &mut Window,
) {
    if source.is_empty() {
        window.paint_quad(gpui::fill(pane, c::QUIET_VEIL()));
        return;
    }
    for (left, top, right, bottom) in [
        (pane.left(), pane.top(), pane.right(), source.top()),
        (pane.left(), source.bottom(), pane.right(), pane.bottom()),
        (pane.left(), source.top(), source.left(), source.bottom()),
        (source.right(), source.top(), pane.right(), source.bottom()),
    ] {
        if right > left && bottom > top {
            window.paint_quad(gpui::fill(
                gpui::Bounds::from_corners(gpui::point(left, top), gpui::point(right, bottom)),
                c::QUIET_VEIL(),
            ));
        }
    }
    let mut outline = gpui::outline(source, c::BORDER_STRONG(), gpui::BorderStyle::default());
    outline.corner_radii = rpx(RADIUS_PANEL).to_pixels(window.rem_size()).into();
    window.paint_quad(outline);
}

/// Deferred after the editor's canvas, so resize and scrolling use this frame's
/// source geometry. The independently constrained panel cannot leave the pane.
struct EditConfirmationTray {
    panel: Option<gpui::AnyElement>,
    placed: Option<gpui::AnyElement>,
    pane: Rc<Cell<gpui::Bounds<gpui::Pixels>>>,
    source: Rc<Cell<gpui::Bounds<gpui::Pixels>>>,
    scroll: gpui::ScrollHandle,
    source_anchor: gpui::ScrollAnchor,
    owner: gpui::WeakEntity<CodexView>,
}
impl gpui::Element for EditConfirmationTray {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<gpui::ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, ()) {
        (
            window.request_layout(
                gpui::Style {
                    position: gpui::Position::Absolute,
                    ..Default::default()
                },
                std::iter::empty(),
                cx,
            ),
            (),
        )
    }
    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: gpui::Bounds<gpui::Pixels>,
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let pane = self.pane.get();
        let source = self
            .source
            .get()
            .intersect(&self.scroll.bounds())
            .intersect(&pane);
        if source.is_empty() {
            // Keep the blocker in place, reveal the initiating action, then
            // attach on the next frame rather than showing a detached fallback.
            self.source_anchor.scroll_to(window, cx);
            let owner = self.owner.clone();
            window.on_next_frame(move |_, cx| {
                let _ = owner.update(cx, |_, cx| cx.notify());
            });
            return;
        }
        let margin = rpx(SPACE_LG).to_pixels(window.rem_size());
        let above = source.center().y >= pane.center().y;
        let available_height = if above {
            source.top() - pane.top() - margin
        } else {
            pane.bottom() - source.bottom() - margin
        };
        let width = rpx(MODAL_W_SM)
            .to_pixels(window.rem_size())
            .min(source.right() - pane.left() - margin)
            .max(gpui::px(0.));
        let Some(panel) = self.panel.take() else {
            return;
        };
        let panel = div()
            .id("codex-edit-confirmation-scroll")
            .w(width)
            .max_h(available_height.max(gpui::px(0.)))
            .overflow_y_scroll()
            .child(panel);
        let mut anchored = gpui::anchored()
            .position_mode(gpui::AnchoredPositionMode::Window)
            .anchor(if above {
                gpui::Anchor::BottomRight
            } else {
                gpui::Anchor::TopRight
            })
            .position(gpui::point(
                source.right(),
                if above { source.top() } else { source.bottom() },
            ))
            .child(panel)
            .into_any_element();
        anchored.prepaint_as_root(
            gpui::Point::default(),
            gpui::size(
                gpui::AvailableSpace::Definite(pane.size.width),
                gpui::AvailableSpace::Definite(pane.size.height),
            ),
            window,
            cx,
        );
        self.placed = Some(anchored);
    }
    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: gpui::Bounds<gpui::Pixels>,
        (): &mut (),
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(panel) = &mut self.placed {
            panel.paint(window, cx);
        }
    }
}
impl gpui::IntoElement for EditConfirmationTray {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl CodexView {
    pub(super) fn edit_confirmation_open(&self) -> bool {
        self.edit_confirmation.is_some()
    }
    fn focus_inline_editor(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = &self.message_editor {
            window.focus(&editor.input.focus_handle(cx), cx);
        }
    }
    fn confirm_inline_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &self.message_editor else {
            return;
        };
        if self.edit_request_pending
            || editor.accepted_prompts.is_some()
            || (editor.input.read(cx).value().trim().is_empty()
                && editor.images.is_empty()
                && editor.reference.is_none())
        {
            return;
        }
        if editor.restored {
            self.send_inline_edit(cx);
            return;
        }
        self.smooth_follow = false;
        if !self
            .scroll
            .bounds()
            .contains(&editor.send_bounds.get().center())
        {
            editor.source_anchor.scroll_to(window, cx);
        }
        self.edit_confirmation = Some((editor.index, editor.message_id.clone()));
        window.focus(&self.edit_cancel_focus, cx);
        cx.notify();
    }
    fn send_inline_edit(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.message_editor else {
            return;
        };
        let draft = editor.input.read(cx).value().to_string();
        let text = editor
            .reference
            .as_ref()
            .map_or_else(|| draft.clone(), |reference| reference.serialize(&draft));
        let images = editor.images.clone();
        editor.accepted_prompts = Some(self.session.read(cx).state.accepted_prompts);
        self.session.update(cx, |session, cx| {
            session.state.clear_error();
            session.send(Action::PromptInline { text, images }, cx);
        });
    }
    fn render_inline_editor(
        &self,
        editor: &MessageEditor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let disabled = self.edit_request_pending
            || editor.accepted_prompts.is_some()
            || self.edit_confirmation.is_some();
        let send_bounds = editor.send_bounds.clone();
        let send_disabled = disabled
            || (editor.input.read(cx).value().trim().is_empty()
                && editor.images.is_empty()
                && editor.reference.is_none());
        div()
            .id("codex-inline-editor")
            .debug_selector(|| "codex-inline-editor".into())
            .w_full()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG))
            .capture_action(cx.listener(
                |this, action: &gpui_component::input::Enter, window, cx| {
                    if action.shift {
                        cx.propagate();
                    } else {
                        // Handle submit before InputState's propagating Enter action:
                        // the editor remains mounted, so fallback text insertion must stop.
                        this.confirm_inline_edit(window, cx);
                        cx.stop_propagation();
                    }
                },
            ))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape"
                    && !this.edit_request_pending
                    && this
                        .message_editor
                        .as_ref()
                        .is_some_and(|editor| editor.accepted_prompts.is_none())
                {
                    if this.edit_confirmation.take().is_none() {
                        this.message_editor = None;
                        window.focus(&this.input.focus_handle(cx), cx);
                    } else {
                        this.focus_inline_editor(window, cx);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .when_some(editor.reference.as_ref(), |column, reference| {
                column.child(
                    div()
                        .id("codex-inline-reference")
                        .max_h(rpx(INLINE_REFERENCE_MAX_H))
                        .overflow_y_scroll()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .children(
                            std::iter::once(div().child("Referenced selection")).chain(
                                reference
                                    .text
                                    .lines()
                                    .map(|line| div().child(line.to_owned())),
                            ),
                        ),
                )
            })
            .child(
                Input::new(&editor.input)
                    .disabled(disabled)
                    .aria_label("Edit sent message")
                    .p_0()
                    .appearance(false)
                    .bordered(false)
                    .focus_bordered(false)
                    .text_color(c::FG()),
            )
            .child(
                div()
                    .id("codex-inline-editor-actions")
                    .flex()
                    .justify_end()
                    .anchor_scroll(Some(editor.source_anchor.clone()))
                    .gap(rpx(SPACE_LG))
                    .child(
                        form_action("codex-cancel-message-edit", "Cancel", false, window, cx)
                            .icon(
                                gpui_component::Icon::default()
                                    .path("icons/close.svg")
                                    .size(rpx(ICON_SM)),
                            )
                            .disabled(disabled)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.message_editor = None;
                                this.edit_confirmation = None;
                                window.focus(&this.input.focus_handle(cx), cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .relative()
                            .flex_shrink_0()
                            .child(
                                form_action("codex-send-message-edit", "Send", true, window, cx)
                                    .relative()
                                    .debug_selector(|| "codex-send-message-edit".into())
                                    .icon(
                                        gpui_component::Icon::default()
                                            .path("icons/arrow-up.svg")
                                            .size(rpx(ICON_SM)),
                                    )
                                    .disabled(send_disabled)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.confirm_inline_edit(window, cx);
                                    })),
                            )
                            .child(
                                gpui::canvas(
                                    move |bounds, _, _| send_bounds.set(bounds),
                                    |_, (), _, _| {},
                                )
                                .absolute()
                                .inset_0(),
                            ),
                    ),
            )
            .into_any_element()
    }
    fn render_edit_confirmation(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let pane = Rc::new(Cell::new(gpui::Bounds::default()));
        let measure_pane = pane.clone();
        let Some((index, message_id)) = self.edit_confirmation.clone() else {
            return div().into_any_element();
        };
        let Some(editor) = self
            .message_editor
            .as_ref()
            .filter(|editor| editor.index == index && editor.message_id == message_id)
        else {
            return div().into_any_element();
        };
        let source = editor.send_bounds.clone();
        let paint_source = source.clone();
        let source_viewport = self.scroll.clone();
        let dialog =
                    div()
                        .id("codex-edit-confirmation")
                        .debug_selector(|| "codex-edit-confirmation".into())
                        .role(gpui::Role::Dialog)
                        .aria_label("edit and restore changes?")
                        .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                            if event.keystroke.key == "escape" {
                                this.edit_confirmation = None;
                                this.focus_inline_editor(window, cx);
                                cx.stop_propagation(); cx.notify();
                            } else if event.keystroke.key == "tab" {
                                let next = if this.edit_cancel_focus.is_focused(window) { &this.edit_confirm_focus } else { &this.edit_cancel_focus };
                                window.focus(next, cx); cx.stop_propagation();
                            }
                        }))
                        .flex().flex_col().gap(rpx(SPACE_LG))
                        .p(rpx(SPACE_2XL))
                        .rounded(rpx(RADIUS_GROUP))
                        .border_1().border_color(c::BORDER()).bg(c::SURFACE_RAISED())
                        .child(div().text_size(rpx(TEXT_TITLE)).child("edit and restore changes?"))
                        .child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child(
                            "Restore tracked and untracked files in this session’s repositories to before this message. This message and all later replies will be removed. Ignored files, commits, and staged changes stay as they are."
                        ))
                        .child(div().flex().flex_wrap().justify_end().gap(rpx(SPACE_LG))
                            .child(form_action("codex-cancel-edit", "Cancel", false, window, cx).icon(gpui_component::Icon::default().path("icons/close.svg").size(rpx(ICON_SM)))
                                .with_focus_handle(self.edit_cancel_focus.clone())
                                .debug_selector(|| "codex-cancel-edit".into())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.edit_confirmation = None;
                                    this.focus_inline_editor(window, cx); cx.notify();
                                })))
                            .child(form_action("codex-confirm-edit", "Edit and restore", true, window, cx).icon(gpui_component::Icon::default().path("icons/restore.svg").size(rpx(ICON_SM)))
                                .with_focus_handle(self.edit_confirm_focus.clone())
                                .debug_selector(|| "codex-confirm-edit".into())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if this.edit_confirmation.take().is_some() {
                                        this.edit_request_pending = true;
                                        this.session.update(cx, |session, cx| {
                                            session.state.clear_error();
                                            session.send(Action::EditMessageInline {index, message_id:message_id.clone()}, cx);
                                        });
                                    }
                                    cx.notify();
                                })))
                        ).into_any_element();
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
                    source_anchor: editor.source_anchor.clone(),
                    owner: cx.entity().downgrade(),
                })),
        )
        .into_any_element()
    }

    fn response_footer(
        &mut self,
        group: &ResponseGroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let key = group.key.clone();
        if !self.message_focus.contains_key(&key) {
            let focus = cx.focus_handle();
            self.subscriptions
                .push(cx.on_focus_in(&focus, window, |_, _, cx| cx.notify()));
            self.subscriptions
                .push(cx.on_focus_out(&focus, window, |_, _, _, cx| cx.notify()));
            self.message_focus.insert(key.clone(), focus);
        }
        let focus = self.message_focus[&key].clone();
        let visible = self.message_hover.contains(&key)
            || focus.contains_focused(window, cx)
            || group.source_keys.iter().any(|source| {
                self.message_focus
                    .get(source)
                    .is_some_and(|focus| focus.contains_focused(window, cx))
            });
        let copied = self
            .copied_message
            .as_ref()
            .is_some_and(|(message, _)| message == &key);
        let mut actions = div()
            .id(gpui::SharedString::from(format!("{}:actions", group.key)))
            .debug_selector(move || "codex-response-footer".into())
            .flex()
            .items_center()
            .gap(rpx(SPACE_MD))
            .h(rpx(CHROME_CONTROL_H));
        if let Some(timestamp) = group.received_at {
            actions = actions.child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_MUTE())
                    .child(relative_message_time(timestamp)),
            );
        }
        let keyboard_key = key.clone();
        let text = group.copy_text.clone();
        let keyboard_text = text.clone();
        let mut button = if copied {
            header_control_frame("codex-message-copy", "Copied", c::BG_HOVER())
        } else {
            header_control("codex-message-copy", "Copy entire response")
        }
        .relative()
        .debug_selector(|| "codex-copy-response".into())
        .tab_index(0)
        .focus_visible(|style| style.bg(c::BG_HOVER()))
        .child(crate::icons::icon(
            if copied { "check" } else { "copy" },
            ICON_MD,
            c::FG_DIM(),
        ))
        .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
            if action_key(event) {
                this.copy_message(keyboard_key.clone(), keyboard_text.clone(), cx);
                cx.stop_propagation();
            }
        }))
        .on_click(cx.listener(move |this, _, _, cx| {
            this.copy_message(key.clone(), text.clone(), cx);
        }));
        if copied {
            button = button.child(
                div()
                    .absolute()
                    .bottom(rpx(CHROME_CONTROL_H + SPACE_SM))
                    .right_0()
                    .child(tooltip("Copied", window).build(window, cx)),
            );
        }
        actions = actions.child(button);
        let hover_key = group.key.clone();
        div()
            .id(gpui::SharedString::from(group.key.clone()))
            .w_full()
            .max_w(rpx(CHAT_MAX_W))
            .mt(rpx(-SPACE_LG))
            .track_focus(&focus)
            .tab_index(0)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.message_hover.insert(hover_key.clone());
                } else {
                    this.message_hover.remove(&hover_key);
                }
                cx.notify();
            }))
            .child(actions.when(!visible && !copied, |actions| actions.opacity(0.0)))
            .into_any_element()
    }

    fn copy_code_block(&mut self, key: String, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        self.copy_generation = self.copy_generation.wrapping_add(1);
        let generation = self.copy_generation;
        self.copied_code_blocks.insert(key.clone(), generation);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(CODE_COPY_FEEDBACK_MS))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.copied_code_blocks.get(&key) == Some(&generation) {
                    this.copied_code_blocks.remove(&key);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn copy_message(&mut self, key: String, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        self.copy_generation = self.copy_generation.wrapping_add(1);
        let generation = self.copy_generation;
        self.copied_message = Some((key, generation));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(MOTION_SLOW_MS * 6))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .copied_message
                    .as_ref()
                    .is_some_and(|(_, current)| *current == generation)
                {
                    this.copied_message = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn request_edit(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let state = &self.session.read(cx).state;
        match state.editable_message(index) {
            Ok(_) => {
                if self.edit_request_pending || self.queue_editor.is_some() {
                    return;
                }
                let item = &state.saved.items[index];
                let id = item["id"].as_str().unwrap_or("").to_owned();
                let canonical = codex::item_text(item);
                let (reference, text) =
                    if let Some((reference, reply)) = reference_prompt(&canonical) {
                        (Some(reference), reply.to_owned())
                    } else {
                        (None, canonical)
                    };
                let images = item["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|part| {
                        part["url"]
                            .as_str()
                            .filter(|_| part["type"] == "image")
                            .map(str::to_owned)
                    })
                    .collect();
                let thread_id = state.saved.thread_id.clone();
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .auto_grow(3, 10)
                        .submit_on_enter(true)
                        .default_value(text)
                });
                let subscription = cx.subscribe_in(&input, window, |this, _, event, window, cx| {
                    if matches!(event, InputEvent::PressEnter { shift: false, .. }) {
                        this.confirm_inline_edit(window, cx);
                    } else if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                });
                self.message_editor = Some(MessageEditor {
                    _subscription: subscription,
                    index,
                    message_id: id,
                    reference,
                    input,
                    images,
                    restored: false,
                    accepted_prompts: None,
                    thread_id,
                    send_bounds: Rc::default(),
                    source_anchor: gpui::ScrollAnchor::for_handle(self.scroll.clone()),
                });
                self.edit_confirmation = None;
                self.close_menu(cx);
                self.focus_inline_editor(window, cx);
                cx.notify();
            }
            Err(error) => self.session.update(cx, |session, cx| {
                session.state.error = Some(error);
                cx.notify();
            }),
        }
    }
    fn close_menu(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        self.popup = None;
        self.model_picker = None;
        self.popup_subscription = None;
        self.composer_menu_scope = None;
        self.composer_menu_launch_config = None;
        self.model_from_card = false;
        self.effort_drag = None;
        self.effort_visual = None;
        self.effort_pending = None;
        cx.notify();
    }
    fn open_menu(&mut self, menu: ComposerMenu, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.session.read(cx).state.clone();
        if menu == ComposerMenu::Access && session_has_yolo_access(&state) {
            self.close_menu(cx);
            return;
        }
        self.composer_menu_scope = Some(composer_scope(&state));
        self.composer_menu_launch_config = Some(
            cx.try_global::<crate::settings::SettingsState>()
                .map_or((false, false), |s| {
                    (
                        s.store.yolo_enabled,
                        s.store.chrome_enabled.unwrap_or(false),
                    )
                }),
        );
        self.effort_drag = None;
        self.effort_visual = None;
        if menu == ComposerMenu::Model {
            self.popup = None;
            self.menu = Some(menu);
            let allow_switch = self.dashboard_tray.is_some() && self.session.read(cx).is_deferred();
            let picker =
                cx.new(|cx| model_picker::ModelPicker::new(&state, allow_switch, window, cx));
            self.popup_subscription = Some(cx.subscribe_in(
                &picker,
                window,
                |this, _, event, window, cx| {
                    if !this.composer_menu_valid(cx) {
                        this.close_menu(cx);
                        return;
                    }
                    match event {
                        model_picker::PickerEvent::Dismiss => {
                            if this.model_from_card {
                                this.model_from_card = false;
                                this.open_menu(ComposerMenu::Settings, window, cx);
                                this.card_model_focus.focus(window, cx);
                            } else {
                                this.close_menu(cx);
                                this.option_focus[0].focus(window, cx);
                            }
                        }
                        model_picker::PickerEvent::Select(provider, model) => {
                            let current = this.session.read(cx).state.saved.provider;
                            if this.dashboard_tray.is_some() && this.session.read(cx).is_deferred()
                            {
                                let settings = &cx.global::<crate::settings::SettingsState>().store;
                                let args = provider.agent().launch_args_for_backend(
                                    true,
                                    settings.yolo_enabled,
                                    settings.chrome_enabled.unwrap_or(false),
                                );
                                let result = this.session.update(cx, |session, cx| {
                                    session.select_draft_model(*provider, model.clone(), args, cx)
                                });
                                if let Err(error) = result {
                                    this.session.update(cx, |session, cx| {
                                        session.state.error = Some(error);
                                        cx.notify();
                                    });
                                    return;
                                }
                                this.input.update(cx, |input, cx| {
                                    input.set_placeholder(
                                        composer_placeholder(*provider),
                                        window,
                                        cx,
                                    );
                                });
                                this.model_from_card = false;
                                this.open_menu(ComposerMenu::Settings, window, cx);
                            } else if *provider == current {
                                this.choose_option(model, window, cx);
                            }
                        }
                    }
                    cx.notify();
                },
            ));
            self.model_picker = Some(picker.clone());
            picker.update(cx, model_picker::ModelPicker::discover);
            cx.notify();
            return;
        }
        self.model_picker = None;
        if matches!(menu, ComposerMenu::Settings | ComposerMenu::Effort) {
            self.popup = None;
            self.popup_subscription = None;
            self.menu = Some(ComposerMenu::Settings);
            self.option_focus[1].focus(window, cx);
            cx.notify();
            return;
        }
        let (choices, _) = Self::menu_choices(&state, menu);
        let selected = match menu {
            ComposerMenu::Model => state.model.as_deref().or(state.resolved_model.as_deref()),
            ComposerMenu::Settings | ComposerMenu::Effort => {
                state.effort.as_deref().or(state.resolved_effort.as_deref())
            }
            ComposerMenu::Access => {
                if state.saved.provider == codex::Provider::Claude {
                    state.access.as_deref().or(state.resolved_access.as_deref())
                } else {
                    state.access.as_deref()
                }
            }
        }
        .map(str::to_owned);
        let index = match menu {
            ComposerMenu::Model => 0,
            ComposerMenu::Settings | ComposerMenu::Effort => 1,
            ComposerMenu::Access => 2,
        };
        let context = self.option_focus[index].clone();
        let owner = cx.entity().downgrade();
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let width =
            COMPOSER_MENU_W.min(f32::from(window.viewport_size().width) / scale - SPACE_3XL);
        let height = COMPOSER_MENU_MAX_H
            .min(f32::from(window.viewport_size().height) / scale - APPBAR_TOTAL_H - SPACE_3XL);
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
        self.menu = Some(menu);
        popup.focus_handle(cx).focus(window, cx);
        self.popup = Some(popup);
        cx.notify();
    }
    fn composer_menu_valid(&self, cx: &App) -> bool {
        self.composer_menu_scope
            .as_ref()
            .is_some_and(|scope| *scope == composer_scope(&self.session.read(cx).state))
            && self.composer_menu_launch_config.is_some_and(|config| {
                config
                    == cx.try_global::<crate::settings::SettingsState>().map_or(
                        (false, false),
                        |s| {
                            (
                                s.store.yolo_enabled,
                                s.store.chrome_enabled.unwrap_or(false),
                            )
                        },
                    )
            })
    }

    fn effort_choices(state: &codex::Snapshot) -> Vec<String> {
        effective_model(state)
            .and_then(|model| model["supportedReasoningEfforts"].as_array())
            .into_iter()
            .flatten()
            .filter_map(|entry| entry["reasoningEffort"].as_str().map(str::to_owned))
            .collect()
    }

    fn effort_model(state: &codex::Snapshot) -> String {
        effective_model(state)
            .and_then(|model| model["model"].as_str())
            .or(state.model.as_deref())
            .or(state.resolved_model.as_deref())
            .unwrap_or_default()
            .to_owned()
    }

    fn effort_index(&self, state: &codex::Snapshot, choices: &[String]) -> Option<usize> {
        if let Some(drag) = &self.effort_drag {
            if drag.model == Self::effort_model(state) && drag.choices == choices {
                return Some(drag.index);
            }
        }
        let pending = self
            .effort_pending
            .as_ref()
            .filter(|(model, _)| *model == Self::effort_model(state));
        let value = pending
            .map(|(_, effort)| effort.as_str())
            .or(state.effort.as_deref())
            .or(state.resolved_effort.as_deref())
            .or_else(|| {
                effective_model(state).and_then(|model| model["defaultReasoningEffort"].as_str())
            });
        value
            .and_then(|value| choices.iter().position(|entry| entry == value))
            .or_else(|| (choices.len() == 1).then_some(0))
    }

    fn sync_effort_visual(&mut self, state: &codex::Snapshot, choices: &[String], cx: &App) {
        let scope = composer_scope(state);
        let model = Self::effort_model(state);
        // Retain a newer local choice until its matching snapshot arrives, then
        // allow subsequent same-model configuration changes to become visible.
        let effective_effort = state.effort.as_deref().or(state.resolved_effort.as_deref());
        if self
            .effort_pending
            .as_ref()
            .is_some_and(|(pending_model, effort)| {
                *pending_model != model
                    || !choices.contains(effort)
                    || effective_effort == Some(effort.as_str())
            })
        {
            self.effort_pending = None;
        }
        let valid = self.effort_visual.as_ref().is_some_and(|visual| {
            visual.scope == scope && visual.model == model && visual.choices == choices
        });
        if !valid {
            self.effort_drag = None;
            let fraction = self
                .effort_index(state, choices)
                .map_or(0.0, |index| effort_fraction(index, choices.len()));
            self.effort_visual = Some(EffortVisual {
                scope,
                model,
                choices: choices.to_vec(),
                from: fraction,
                target: fraction,
                started: None,
                duration: std::time::Duration::from_millis(MOTION_BASE_MS),
            });
        } else if self.effort_drag.is_none() {
            // Provider acknowledgments can change effort with the same catalog.
            // Never replace an active drag's continuous pointer target.
            if let Some(index) = self.effort_index(state, choices) {
                self.move_effort_visual(effort_fraction(index, choices.len()), MOTION_BASE_MS, cx);
            }
        }
        if cx.reduce_motion() {
            if let Some(visual) = &mut self.effort_visual {
                visual.from = visual.target;
                visual.started = None;
            }
        }
    }

    fn move_effort_visual(&mut self, fraction: f32, duration: u64, cx: &App) {
        let now = cx.background_executor().now();
        if let Some(visual) = &mut self.effort_visual {
            let target = fraction.clamp(0.0, 1.0);
            if cx.reduce_motion() {
                visual.from = target;
                visual.target = target;
                visual.started = None;
            } else if visual.target != target {
                // Retarget from the clock-sampled position, never from the old stop.
                visual.from = visual.position(now);
                visual.target = target;
                visual.started = Some(now);
                visual.duration = std::time::Duration::from_millis(duration);
            }
        }
    }

    fn send_effort(&mut self, value: &str, cx: &mut Context<Self>) {
        if !self.composer_menu_valid(cx) {
            self.close_menu(cx);
            return;
        }
        let state = &self.session.read(cx).state;
        if !Self::effort_choices(state)
            .iter()
            .any(|choice| choice == value)
        {
            return;
        }
        let choices = Self::effort_choices(state);
        let model = Self::effort_model(state);
        if let Some(index) = choices.iter().position(|choice| choice == value) {
            self.move_effort_visual(effort_fraction(index, choices.len()), MOTION_BASE_MS, cx);
        }
        self.effort_pending = Some((model, value.to_owned()));
        self.session.update(cx, |session, cx| {
            session.send(
                Action::Configure {
                    model: None,
                    effort: Some(value.to_owned()),
                    access: None,
                },
                cx,
            );
        });
        cx.notify();
    }

    fn preview_effort_at(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        moving: bool,
        cx: &mut Context<Self>,
    ) {
        let bounds = self.effort_bounds.get();
        if bounds.size.width <= gpui::px(0.) {
            return;
        }
        let state = &self.session.read(cx).state;
        let valid = self.effort_drag.as_ref().is_some_and(|drag| {
            drag.scope == composer_scope(state)
                && drag.model == Self::effort_model(state)
                && drag.choices == Self::effort_choices(state)
        });
        if !valid {
            self.effort_drag = None;
            self.effort_visual = None;
            cx.notify();
            return;
        }
        let fraction =
            (f32::from(position.x - bounds.left()) / f32::from(bounds.size.width)).clamp(0.0, 1.0);
        if let Some(drag) = &mut self.effort_drag {
            drag.index = effort_stop(fraction, drag.choices.len());
        }
        self.move_effort_visual(
            fraction,
            if moving {
                MOTION_FAST_MS
            } else {
                MOTION_BASE_MS
            },
            cx,
        );
        cx.notify();
    }

    fn finish_effort_drag(&mut self, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        if self.effort_drag.is_none() {
            return;
        }
        self.preview_effort_at(position, true, cx);
        if let Some(drag) = self.effort_drag.take() {
            if let Some(value) = drag.choices.get(drag.index) {
                self.send_effort(value, cx);
            }
            cx.notify();
        }
    }

    fn reset_model_effort(&mut self, cx: &mut Context<Self>) {
        if !self.composer_menu_valid(cx) {
            self.close_menu(cx);
            return;
        }
        self.effort_drag = None;
        self.effort_visual = None;
        self.effort_pending = None;
        self.session
            .update(cx, |session, cx| session.send(Action::ResetModelEffort, cx));
        cx.notify();
    }

    fn send_speed(&mut self, speed: codex::Speed, cx: &mut Context<Self>) {
        if !self.composer_menu_valid(cx) {
            self.close_menu(cx);
            return;
        }
        if self
            .session
            .read(cx)
            .state
            .speed_unavailable(speed)
            .is_some()
        {
            return;
        }
        self.session
            .update(cx, |session, cx| session.send(Action::SetSpeed(speed), cx));
        cx.notify();
    }

    fn speed_controls(&self, state: &codex::Snapshot, cx: &mut Context<Self>) -> gpui::AnyElement {
        let selected = state.saved.composer.speed;
        let mut choices = preference_choices().w_full().bg(c::BG());
        for (index, speed) in [
            codex::Speed::Standard,
            codex::Speed::Fast,
            codex::Speed::Ultrafast,
        ]
        .into_iter()
        .enumerate()
        {
            if state.saved.provider == codex::Provider::Claude && speed == codex::Speed::Ultrafast {
                continue;
            }
            let unavailable = state.speed_unavailable(speed);
            let id = format!("codex-speed-{index}");
            choices =
                choices.child(
                    settings_control(
                        id,
                        format!(
                            "{} speed{}",
                            speed.label(),
                            if unavailable.is_some() {
                                ": unavailable"
                            } else {
                                ""
                            }
                        ),
                        selected == Some(speed),
                        unavailable.is_none(),
                    )
                    .debug_selector(move || format!("codex-speed-{index}"))
                    .track_focus(&self.speed_focus[index])
                    .aria_description(unavailable.unwrap_or(
                        "Requested speed for the next turn; provider availability applies",
                    ))
                    .border_0()
                    .flex_1()
                    .justify_center()
                    .bg(if selected == Some(speed) {
                        c::BG_HOVER()
                    } else {
                        c::BG()
                    })
                    .child(speed.label())
                    .when(unavailable.is_none(), |control| {
                        control
                            .on_click(cx.listener(move |this, _, _, cx| this.send_speed(speed, cx)))
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, window, cx| {
                                    if action_key(event) {
                                        this.send_speed(speed, cx);
                                        window.prevent_default();
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                    }),
                );
        }
        let mut notes = Vec::new();
        if selected.is_none() {
            notes.push("Using this conversation’s inherited provider speed. Choose a speed to override it.".to_owned());
        }
        if let Some(speed) = selected.filter(|speed| *speed != codex::Speed::Standard) {
            if let Some(reason) = state.speed_unavailable(speed) {
                notes.push(format!(
                    "Using Standard for the next turn. {} preference retained. {reason}",
                    speed.label()
                ));
            }
        }
        if state.saved.provider == codex::Provider::Codex {
            notes.push(
                match selected {
                    Some(codex::Speed::Ultrafast) => {
                        "ChatGPT plans: Ultrafast uses 8× included usage or 6× purchased credits. API pricing varies."
                    }
                    _ => "ChatGPT plans: Fast uses 2.5× included usage or 2× purchased credits. API pricing varies.",
                }
                .into(),
            );
            if state.speed_unavailable(codex::Speed::Ultrafast).is_some() {
                notes.push("Ultrafast unavailable for this model or account.".into());
            }
        } else {
            notes.push("Fast uses paid usage credits on subscriptions; included allowance does not cover it. Account and organization eligibility apply.".into());
            if state.speed_unavailable(codex::Speed::Fast).is_some() {
                notes.push(
                    "Fast requires a supported Opus model. Choose a model to enable it.".into(),
                );
            }
            if selected == Some(codex::Speed::Fast) {
                if let Some(reported) = state
                    .reported_speed
                    .as_deref()
                    .filter(|s| *s == "off" || *s == "cooldown")
                {
                    notes.push(format!(
                        "Claude last reported Fast {reported}{}. Fast preference retained.",
                        state
                            .speed_disabled_reason
                            .as_ref()
                            .map(|r| format!(": {}", speed_disabled_explanation(r)))
                            .unwrap_or_default()
                    ));
                }
            }
        }
        div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG))
            .pt(rpx(SPACE_2XL))
            .border_t_1()
            .border_color(c::BORDER_SOFT())
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(div().text_color(c::FG()).child("Speed"))
                    .child(div().text_color(c::FG_MUTE()).child("Next turn")),
            )
            .child(choices)
            .children(notes.into_iter().map(|note| {
                div()
                    .text_size(rpx(TEXT_MICRO))
                    .text_color(c::FG_DIM())
                    .child(note)
            }))
            .into_any_element()
    }

    fn model_effort_card(
        &mut self,
        state: &codex::Snapshot,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let choices = Self::effort_choices(state);
        self.sync_effort_visual(state, &choices, cx);
        let selected = self.effort_index(state, &choices);
        let effort = selected.and_then(|index| choices.get(index)).map_or_else(
            || {
                if choices.is_empty() {
                    "Effort unavailable".into()
                } else {
                    "Default effort".into()
                }
            },
            |value| effort_label(value),
        );
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let width =
            MODEL_EFFORT_CARD_W.min(f32::from(window.viewport_size().width) / scale - SPACE_3XL);
        let margin = rpx(SPACE_LG).to_pixels(window.rem_size());
        let trigger_top = self.option_bounds[0].get().top();
        let available_height = if trigger_top > margin * 2.0 {
            trigger_top - margin * 2.0
        } else {
            window.viewport_size().height - margin * 2.0
        };
        let model_name = composer_model_name(state);
        let mut model_run = window.text_style().to_run(model_name.len());
        model_run.font.family = crate::fonts::UI_FAMILY.into();
        // Reserve the card padding, flanking controls, row gaps, chevron and seam.
        let model_budget =
            rpx(width - SPACE_2XL * 2.0 - CONTROL_H * 2.0 - SPACE_SM * 2.0 - ICON_XS - SPACE_XS)
                .to_pixels(window.rem_size())
                - gpui::px(SPACE_XS);
        let model_width = truncation_safe_label_width(
            &model_name,
            rpx(TEXT_SMALL).to_pixels(window.rem_size()),
            &model_run,
            window.text_system(),
        )
        .min(model_budget.max(gpui::px(0.)));
        let now = cx.background_executor().now();
        let fraction = self
            .effort_visual
            .as_ref()
            .map_or(0.0, |visual| visual.position(now));
        if !cx.reduce_motion()
            && self
                .effort_visual
                .as_ref()
                .is_some_and(|visual| visual.active(now))
        {
            window.request_animation_frame();
        }
        let track_bounds = self.effort_bounds.clone();
        let pointer_move = cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
            if this.effort_drag.is_some() {
                if event.pressed_button == Some(gpui::MouseButton::Left) {
                    this.preview_effort_at(event.position, true, cx);
                } else {
                    this.effort_drag = None;
                    this.effort_visual = None;
                    cx.notify();
                }
            }
        });
        let pointer_up = cx.listener(|this, event: &gpui::MouseUpEvent, _, cx| {
            if event.button == gpui::MouseButton::Left {
                this.finish_effort_drag(event.position, cx);
            }
        });
        let track = div()
            .relative()
            .w_full()
            .h(rpx(CONTROL_H))
            .rounded(rpx(RADIUS_FULL))
            .bg(c::BG_HOVER())
            .child(
                div()
                    .absolute()
                    .left(rpx(ICON_MD))
                    .right(rpx(ICON_MD))
                    .h_full()
                    .child(
                        gpui::canvas(
                            move |bounds, _, _| track_bounds.set(bounds),
                            move |_, (), window, _| {
                                // Capture an owned drag even over other controls or outside the track.
                                window.on_mouse_event(
                                    move |event: &gpui::MouseMoveEvent, phase, window, cx| {
                                        if phase == gpui::DispatchPhase::Capture {
                                            pointer_move(event, window, cx);
                                        }
                                    },
                                );
                                window.on_mouse_event(
                                    move |event: &gpui::MouseUpEvent, phase, window, cx| {
                                        if phase == gpui::DispatchPhase::Capture {
                                            pointer_up(event, window, cx);
                                        }
                                    },
                                );
                            },
                        )
                        .absolute()
                        .size_full(),
                    )
                    .when(selected.is_some(), |path| {
                        path.child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .h_full()
                                .w(gpui::relative(fraction))
                                .child(
                                    div()
                                        .id("codex-effort-fill")
                                        .debug_selector(|| "codex-effort-fill".into())
                                        .absolute()
                                        .left(-rpx(ICON_MD))
                                        // Keep both caps full-sized beneath the thumb, even
                                        // at the first stop where a short fill would flatten.
                                        .right(-rpx(ICON_MD))
                                        .h_full()
                                        .rounded(rpx(RADIUS_FULL))
                                        .bg(c::BLUE()),
                                ),
                        )
                    })
                    .children(choices.iter().enumerate().map(|(index, _)| {
                        div()
                            .absolute()
                            .left(gpui::relative(effort_fraction(index, choices.len())))
                            .top(rpx(SPACE_XL))
                            .ml(-rpx(SPACE_XS))
                            .size(rpx(SPACE_SM))
                            .rounded(rpx(RADIUS_FULL))
                            .bg(c::FG_MUTE())
                    }))
                    .when_some(selected, |path, _| {
                        path.child(
                            div()
                                .id("codex-effort-thumb")
                                .debug_selector(|| "codex-effort-thumb".into())
                                .absolute()
                                .left(gpui::relative(fraction))
                                .top(-rpx(SPACE_XS))
                                .ml(-rpx(ICON_MD))
                                .size(rpx(ICON_BTN_W))
                                .rounded(rpx(RADIUS_FULL))
                                .bg(c::FG()),
                        )
                    }),
            );
        let slider = div()
            .id("codex-effort-slider")
            .debug_selector(|| "codex-effort-slider".into())
            .role(gpui::Role::Slider)
            .aria_label("Reasoning effort")
            .aria_value(effort.clone())
            .aria_description(if choices.len() < 2 {
                "This model has no adjustable effort range"
            } else {
                "Applies to the next turn. Use arrow keys, Home or End"
            })
            .aria_min_numeric_value(0.0)
            .aria_max_numeric_value(choices.len().saturating_sub(1) as f64)
            .aria_numeric_value(selected.unwrap_or(0) as f64)
            .aria_numeric_value_step(1.0)
            .track_focus(&self.option_focus[1])
            .tab_index(0)
            .h(rpx(ICON_BTN_W))
            .w_full()
            .flex()
            .items_center()
            .rounded(rpx(RADIUS_FULL))
            .focus_visible(|style| style.bg(c::BG_HOVER()))
            .when(choices.len() < 2, |slider| slider.opacity(OPACITY_DISABLED))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    if !this.composer_menu_valid(cx) {
                        return;
                    }
                    let state = &this.session.read(cx).state;
                    let choices = Self::effort_choices(state);
                    if choices.len() > 1 {
                        this.effort_drag = Some(EffortDrag {
                            scope: composer_scope(state),
                            model: Self::effort_model(state),
                            choices,
                            index: 0,
                        });
                        this.preview_effort_at(event.position, false, cx);
                        this.option_focus[1].focus(window, cx);
                    }
                    window.prevent_default();
                    cx.stop_propagation();
                }),
            )
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                let state = &this.session.read(cx).state;
                let choices = Self::effort_choices(state);
                let index = this.effort_index(state, &choices).unwrap_or(0);
                if let Some(index) = effort_key_index(&event.keystroke.key, index, choices.len()) {
                    this.effort_drag = None;
                    this.send_effort(&choices[index], cx);
                    cx.stop_propagation();
                }
            }))
            .child(track);
        div()
            .id("codex-model-effort-card")
            .debug_selector(|| "codex-model-effort-card".into())
            .role(gpui::Role::Group)
            .track_focus(&self.card_focus)
            .aria_label("Model, reasoning effort and speed")
            .w(rpx(width))
            .max_h(available_height)
            .overflow_y_scroll()
            .p(rpx(SPACE_2XL))
            .rounded(rpx(RADIUS_PANEL))
            .border_1()
            .border_color(c::BORDER())
            .bg(c::BG_RAIL())
            .text_size(rpx(TEXT_SMALL))
            .text_color(c::FG_DIM())
            .flex()
            .flex_col()
            .gap(rpx(SPACE_2XL))
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                if !this.option_bounds[0].get().contains(&event.position) {
                    this.close_menu(cx);
                }
            }))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(rpx(SPACE_SM))
                    .child(
                        div()
                            .w(rpx(CONTROL_H))
                            .h(rpx(CONTROL_H))
                            .flex()
                            .items_center()
                            .child(crate::icons::icon("lightning", ICON_MD, c::FG_MUTE())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .items_center()
                            .child(
                                div()
                                    .text_size(rpx(TEXT_TITLE))
                                    .text_color(c::FG())
                                    .child(effort),
                            )
                            .child(
                                header_control_frame(
                                    "codex-card-model",
                                    format!("Choose model: {}", composer_model_name(state)),
                                    c::BG_HOVER(),
                                )
                                .track_focus(&self.card_model_focus)
                                .tab_index(0)
                                .w_full()
                                .min_w_0()
                                .gap(rpx(SPACE_XS))
                                .focus_visible(|style| style.bg(c::BG_HOVER()))
                                .child(
                                    div()
                                        .id("codex-card-model-label")
                                        .debug_selector(|| "codex-card-model-label".into())
                                        .w(model_width)
                                        .flex_shrink_0()
                                        .truncate()
                                        .child(model_name),
                                )
                                .child(crate::icons::icon("chev-right", ICON_XS, c::FG_MUTE()))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.model_from_card = true;
                                    this.effort_pending = None;
                                    this.open_menu(ComposerMenu::Model, window, cx);
                                }))
                                .on_key_down(cx.listener(
                                    |this, event: &gpui::KeyDownEvent, window, cx| {
                                        if action_key(event) {
                                            this.model_from_card = true;
                                            this.effort_pending = None;
                                            this.open_menu(ComposerMenu::Model, window, cx);
                                            cx.stop_propagation();
                                        }
                                    },
                                )),
                            ),
                    )
                    .child(
                        header_control(
                            "codex-model-effort-reset",
                            "Reset model and effort to configured values",
                        )
                        .debug_selector(|| "codex-model-effort-reset".into())
                        .track_focus(&self.card_reset_focus)
                        .tab_index(0)
                        .focus_visible(|style| style.bg(c::BG_HOVER()))
                        .child(crate::icons::icon("reset", ICON_MD, c::FG_DIM()))
                        .on_click(cx.listener(|this, _, _, cx| this.reset_model_effort(cx)))
                        .on_key_down(cx.listener(
                            |this, event: &gpui::KeyDownEvent, _, cx| {
                                if action_key(event) {
                                    this.reset_model_effort(cx);
                                    cx.stop_propagation();
                                }
                            },
                        )),
                    ),
            )
            .child(slider)
            .child(self.speed_controls(state, cx))
            .into_any_element()
    }

    fn option_triggers(
        &self,
        state: &codex::Snapshot,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let model = composer_model_name(state);
        let effort = state
            .effort
            .as_deref()
            .or(state.resolved_effort.as_deref())
            .map_or_else(|| "Default effort".into(), effort_label);
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
            Some("default" | "manual") => "Ask for permission",
            Some("auto") => "Automatic permissions",
            Some("acceptEdits") => "Accept edits",
            Some("plan") => "Plan mode",
            Some("dontAsk") => "Deny unapproved tools",
            Some("bypassPermissions") => "Bypass permissions",
            _ => "Configured access",
        };
        let access = if state.saved.provider == codex::Provider::Codex && state.access.is_none() {
            state.configured_access_summary()
        } else {
            access.to_owned()
        };
        let mut controls = Vec::new();
        for (index, menu, id, name, icon, label) in [
            (
                0,
                ComposerMenu::Settings,
                "codex-model",
                "Choose model, reasoning effort and speed",
                state.saved.provider.agent().icon_name(),
                format!(
                    "{model} · {effort} · {}",
                    state
                        .saved
                        .composer
                        .speed
                        .map_or("Inherited speed", codex::Speed::label)
                ),
            ),
            (
                2,
                ComposerMenu::Access,
                "codex-access",
                "Choose access level",
                access_icon,
                access.clone(),
            ),
        ] {
            if index > 0 {
                controls.push(
                    div()
                        .h(rpx(ICON_LG))
                        .border_l_1()
                        .border_color(c::BORDER_SOFT())
                        .into_any_element(),
                );
            }
            if menu == ComposerMenu::Access && session_has_yolo_access(state) {
                controls.push(
                    div()
                        .id("codex-yolo-access")
                        .debug_selector(|| "codex-yolo-access".into())
                        .flex()
                        .items_center()
                        .flex_shrink_0()
                        .min_h(rpx(CHROME_CONTROL_H))
                        .px(rpx(SPACE_SM))
                        .gap(rpx(SPACE_SM))
                        .text_color(c::FG_DIM())
                        .text_size(rpx(TEXT_SMALL))
                        .child(crate::icons::icon("lock-open", ICON_SM, c::FG_DIM()))
                        .child("Yolo mode · change in Settings")
                        .into_any_element(),
                );
                continue;
            }
            let bounds = self.option_bounds[index].clone();
            let summary_id = if menu == ComposerMenu::Settings {
                "codex-model-effort-summary"
            } else {
                "codex-access-summary"
            };
            // GPUI's intrinsic flex text measurement can undercount glyph advances,
            // causing an ellipsis even when the toolbar has space for this label.
            let mut label_run = window.text_style().to_run(label.len());
            label_run.font.family = crate::fonts::UI_FAMILY.into();
            let label_width = truncation_safe_label_width(
                &label,
                rpx(TEXT_SMALL).to_pixels(window.rem_size()),
                &label_run,
                window.text_system(),
            );
            let detail = if menu == ComposerMenu::Access
                && state.saved.provider == codex::Provider::Codex
                && state.access.is_none()
            {
                format!(
                    "Sandbox: {}\nApprovals: {}",
                    state
                        .sandbox
                        .as_ref()
                        .map_or_else(|| "provider configured".into(), ToString::to_string),
                    state
                        .approval_policy
                        .as_ref()
                        .map_or_else(|| "provider configured".into(), ToString::to_string)
                )
            } else if menu == ComposerMenu::Access {
                access.clone()
            } else {
                format!(
                    "{name}. Speed is your next-turn preference; provider availability applies."
                )
            };
            let control = header_control_with_tooltip(id, name, move |window, cx| {
                tooltip(detail.clone(), window).build(window, cx)
            });
            controls.push(
                control
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
                    .when(
                        self.menu == Some(menu)
                            || (menu == ComposerMenu::Settings && self.model_from_card),
                        |button| button.bg(c::BG_HOVER()),
                    )
                    .focus_visible(|style| style.bg(c::BG_HOVER()))
                    .child(
                        div()
                            .flex_shrink_0()
                            .size(rpx(ICON_SM))
                            .child(crate::icons::icon(icon, ICON_SM, c::FG_DIM())),
                    )
                    .when(menu == ComposerMenu::Settings, |button| {
                        button.max_w_full().min_h(rpx(CHROME_CONTROL_H))
                    })
                    .when(menu == ComposerMenu::Access, |button| {
                        button.max_w_full().h_auto().min_h(rpx(CHROME_CONTROL_H))
                    })
                    .child(
                        div()
                            .id(summary_id)
                            .debug_selector(move || summary_id.into())
                            .when(menu == ComposerMenu::Settings, |label| {
                                label
                                    .w(label_width
                                        .min(rpx(MODEL_EFFORT_CARD_W).to_pixels(window.rem_size())))
                                    .max_w(rpx(MODEL_EFFORT_CARD_W))
                                    .flex_shrink_0()
                                    .truncate()
                            })
                            .when(menu != ComposerMenu::Settings, |label| {
                                label
                                    .w(label_width)
                                    .min_w_0()
                                    .flex_shrink_1()
                                    .whitespace_normal()
                            })
                            .child(label),
                    )
                    .child(
                        div()
                            .id(if menu == ComposerMenu::Access {
                                "codex-access-indicator"
                            } else {
                                "codex-model-indicator"
                            })
                            .flex_shrink_0()
                            .size(rpx(ICON_SM))
                            .when(menu == ComposerMenu::Access, |indicator| {
                                indicator.debug_selector(|| "codex-access-indicator".into())
                            })
                            .child(crate::icons::icon("chev-down", ICON_SM, c::FG_DIM())),
                    )
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
            ComposerMenu::Settings | ComposerMenu::Effort => {
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
                        ("default", "Ask for permission · provider rules apply"),
                        ("manual", "Ask for permission · provider rules apply"),
                        (
                            "auto",
                            "Automatic permissions · provider evaluates tool risk",
                        ),
                        ("acceptEdits", "Accept edits · ask before other tools"),
                        ("plan", "Plan mode · explore before implementing"),
                        ("dontAsk", "Deny tools that require permission"),
                        (
                            "bypassPermissions",
                            "Bypass approval checks · provider restrictions still apply",
                        ),
                    ]
                    .into_iter()
                    .filter(|(mode, _)| {
                        grove_core::claude::can_select_permission_mode(&state.saved, mode)
                            && state
                                .permission_modes
                                .iter()
                                .any(|supported| supported == mode)
                    })
                    .collect()
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
                "Access · explicit selection overrides provider defaults"
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
        if self.menu.is_some() && !self.composer_menu_valid(cx) {
            self.close_menu(cx);
            return;
        }
        let return_to_card = self.model_from_card;
        self.popup_subscription = None;
        if value == "__configured" {
            self.close_menu(cx);
            self.session.update(cx, |session, cx| {
                session.send(
                    if return_to_card {
                        Action::ResetModelEffort
                    } else {
                        Action::ResetConfiguration
                    },
                    cx,
                );
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
        if return_to_card {
            self.model_from_card = false;
            self.open_menu(ComposerMenu::Settings, window, cx);
        } else if self.popup.is_none() {
            self.input.focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }
    fn remove_reference(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.queue_editor.as_mut() {
            editor.reference = None;
        } else {
            self.set_reference(None, cx);
        }
        self.reference_expanded = false;
        self.input.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn set_reference(&mut self, reference: Option<codex::ReferenceDraft>, cx: &mut Context<Self>) {
        self.reference.clone_from(&reference);
        self.reference_expanded = false;
        self.session.update(cx, |session, cx| {
            session.state.saved.draft_reference.clone_from(&reference);
            session.send(Action::DraftReference(reference), cx);
        });
        cx.notify();
    }

    fn selection_reference(
        &self,
        snapshot: &TextSelectionSnapshot,
        state: &codex::Snapshot,
    ) -> Option<codex::ReferenceDraft> {
        if snapshot.is_selecting
            || snapshot.text.trim().is_empty()
            || snapshot.spans.is_empty()
            || self.message_editor.is_some()
            || self.queue_editor.is_some()
            || self.edit_confirmation.is_some()
            || self.menu.is_some()
            || self.popup.is_some()
        {
            return None;
        }
        let mut sources = Vec::new();
        for span in &snapshot.spans {
            let source = state.saved.items.iter().find_map(|item| {
                let id = item["id"].as_str()?;
                let role = match item["type"].as_str()? {
                    "userMessage" => "User",
                    "agentMessage" => "Assistant",
                    _ => return None,
                };
                let key = format!("{}:{}", state.saved.thread_id.as_deref().unwrap_or(""), id);
                let owner = self.selection_views.get(&key)?;
                (owner.entity_id() == span.owner).then(|| codex::ReferenceSource {
                    message_id: id.to_owned(),
                    role: role.to_owned(),
                })
            })?;
            if sources.last() != Some(&source) {
                sources.push(source);
            }
        }
        Some(codex::ReferenceDraft {
            id: String::new(),
            thread_id: state.saved.thread_id.clone(),
            sources,
            text: snapshot.text.clone(),
        })
    }

    fn attach_selection(
        &mut self,
        expected: &TextSelectionSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        static NEXT_REFERENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let Some(current) = window.text_selection_snapshot(cx) else {
            return;
        };
        if &current != expected {
            return;
        }
        let state = self.session.read(cx).state.shared();
        let Some(mut reference) = self.selection_reference(&current, &state) else {
            return;
        };
        reference.id = format!(
            "{}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            NEXT_REFERENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        self.set_reference(Some(reference), cx);
        window.clear_text_selection(cx);
        self.selection_dismissed = None;
        self.set_composer_collapsed(false, cx);
        self.input.focus_handle(cx).focus(window, cx);
    }

    fn render_selection_reply(
        &mut self,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        window.set_text_selection_action_bounds(cx.entity_id(), None, cx);
        let Some(snapshot) = window.text_selection_snapshot(cx) else {
            self.selection_dismissed = None;
            return None;
        };
        if snapshot.is_selecting {
            self.selection_dismissed = None;
            return None;
        }
        if self.selection_dismissed.as_ref() == Some(&snapshot) {
            return None;
        }
        self.selection_reference(&snapshot, state)?;
        let pane = self.transcript_bounds.get();
        if !pane.contains(&snapshot.endpoint) {
            return None;
        }
        let control = rpx(CHROME_CONTROL_H).to_pixels(window.rem_size());
        let gap = rpx(SPACE_SM).to_pixels(window.rem_size());
        let mut left = snapshot.endpoint.x + gap;
        if left + control > pane.right() {
            left = snapshot.endpoint.x - gap - control;
        }
        left = left
            .max(pane.left())
            .min((pane.right() - control).max(pane.left()));
        let top = snapshot
            .endpoint
            .y
            .min(pane.bottom() - control)
            .max(pane.top());
        let keyboard = snapshot.clone();
        let action_owner = cx.entity_id();
        Some(
            header_control("codex-selection-reply", "Reply to selection")
                .debug_selector(|| "codex-selection-reply".into())
                .absolute()
                .left(left - pane.left())
                .top(top - pane.top())
                .bg(c::SURFACE_RAISED())
                .border_1()
                .border_color(c::BORDER_SOFT())
                .track_focus(
                    &self
                        .selection_reply_focus
                        .clone()
                        .tab_index(0)
                        .tab_stop(true),
                )
                .tab_index(0)
                .focus_visible(|style| style.bg(c::BG_HOVER()))
                .child(
                    gpui::canvas(
                        move |bounds, window, cx| {
                            window.set_text_selection_action_bounds(action_owner, Some(bounds), cx);
                        },
                        |_, (), _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
                .child(crate::icons::icon("reply", ICON_MD, c::FG()))
                .on_key_down(
                    cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                        if action_key(event) {
                            this.attach_selection(&keyboard, window, cx);
                            cx.stop_propagation();
                        }
                    }),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.attach_selection(&snapshot, window, cx);
                    cx.stop_propagation();
                }))
                .into_any_element(),
        )
    }

    fn render_reference_preview(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let quote = if let Some(editor) = self.queue_editor.as_ref() {
            editor.reference.as_ref()?
        } else {
            self.reference.as_ref()?
        };
        let full = quote.text.clone();
        let needs_disclosure =
            full.lines().count() > 3 || full.chars().count() > REFERENCE_PREVIEW_CHARS;
        let preview = if self.reference_expanded {
            full.clone()
        } else {
            let lines: Vec<_> = full.lines().take(3).collect();
            let mut preview: String = lines
                .join("\n")
                .chars()
                .take(REFERENCE_PREVIEW_CHARS)
                .collect();
            if needs_disclosure {
                preview.push('…');
            }
            preview
        };
        let provider = self.session.read(cx).state.saved.provider;
        let mut roles = Vec::new();
        for source in &quote.sources {
            let role = if source.role == "Assistant" {
                provider.label()
            } else {
                "You"
            };
            if !roles.contains(&role) {
                roles.push(role);
            }
        }
        let sources = roles.join(" · ");
        Some(
            div()
                .id("codex-reference-preview")
                .debug_selector(|| "codex-reference-preview".into())
                .w_full()
                .min_w_0()
                .flex_shrink_0()
                .px(rpx(SPACE_2XL))
                .pt(rpx(SPACE_LG))
                .pb(rpx(SPACE_LG))
                .rounded_t(rpx(RADIUS_PANEL))
                .border_1()
                .border_b_0()
                .border_color(c::BORDER_SOFT())
                .bg(c::BG_RAIL())
                .flex()
                .flex_col()
                .gap(rpx(SPACE_SM))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_SM))
                        .child(crate::icons::icon("reply", ICON_MD, c::FG_DIM()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(rpx(TEXT_MICRO))
                                .text_color(c::FG_DIM())
                                .child(format!("Replying to {sources}")),
                        )
                        .child(
                            header_control("codex-remove-reference", "Remove reference")
                                .debug_selector(|| "codex-remove-reference".into())
                                .track_focus(
                                    &self
                                        .reference_remove_focus
                                        .clone()
                                        .tab_index(0)
                                        .tab_stop(true),
                                )
                                .tab_index(0)
                                .focus_visible(|style| style.bg(c::SURFACE_RAISED()))
                                .child(crate::icons::icon("close", ICON_MD, c::FG_DIM()))
                                .on_key_down(cx.listener(
                                    |this, event: &gpui::KeyDownEvent, window, cx| {
                                        if action_key(event) {
                                            this.remove_reference(window, cx);
                                            cx.stop_propagation();
                                        }
                                    },
                                ))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.remove_reference(window, cx);
                                })),
                        ),
                )
                .child(
                    div()
                        .id("codex-reference-text")
                        .debug_selector(|| "codex-reference-text".into())
                        .min_w_0()
                        .max_h(rpx(if self.reference_expanded {
                            REFERENCE_EXPANDED_H
                        } else {
                            REFERENCE_PREVIEW_H
                        }))
                        .overflow_y_scroll()
                        .border_l_2()
                        .border_color(c::BORDER())
                        .pl(rpx(SPACE_LG))
                        .text_size(rpx(TEXT_BODY))
                        .text_color(c::FG())
                        .children(
                            preview
                                .lines()
                                .map(|line| div().child(line.to_owned()))
                                .collect::<Vec<_>>(),
                        ),
                )
                .when(needs_disclosure, |preview| {
                    preview.child(
                        transcript_action(
                            "codex-reference-disclosure",
                            if self.reference_expanded {
                                "Show less"
                            } else {
                                "Show full quote"
                            },
                            false,
                            window,
                            cx,
                        )
                        .debug_selector(|| "codex-reference-disclosure".into())
                        .with_focus_handle(self.reference_disclosure_focus.clone())
                        .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                            if action_key(event) {
                                this.reference_expanded = !this.reference_expanded;
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }))
                        .icon(
                            gpui_component::Icon::default()
                                .path(if self.reference_expanded {
                                    "icons/arrow-up.svg"
                                } else {
                                    "icons/chev-down.svg"
                                })
                                .size(rpx(ICON_SM)),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.reference_expanded = !this.reference_expanded;
                            cx.notify();
                        })),
                    )
                })
                .into_any_element(),
        )
    }

    fn run_command(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.reference.is_some() {
            return false;
        }
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
            session.state.clear_error();
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
    let start = active_work_start(state);
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

/// New turns can start without a new user message (for example, manual compaction).
fn active_work_start(state: &codex::Snapshot) -> usize {
    state
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
        )
}

/// Presentation-only response boundaries; provider work periods never split a response.
#[derive(Debug)]
struct ResponseGroup {
    key: String,
    end: usize,
    prose: Vec<usize>,
    source_keys: Vec<String>,
    copy_text: String,
    received_at: Option<u64>,
}

fn response_groups(items: &[serde_json::Value], thread_id: Option<&str>) -> Vec<ResponseGroup> {
    let mut groups = Vec::new();
    let mut start = 0;
    for end in (0..=items.len())
        .filter(|index| *index == items.len() || items[*index]["type"] == "userMessage")
    {
        let prose = (start..end)
            .filter(|index| {
                items[*index]["type"] == "agentMessage"
                    && !codex::item_text(&items[*index]).trim().is_empty()
            })
            .collect::<Vec<_>>();
        if let Some(last) = prose.last() {
            let first = (start..end)
                .find(|index| items[*index]["type"] == "agentMessage")
                .unwrap_or(*last);
            let source_id = |index: usize| {
                items[index]["id"]
                    .as_str()
                    .map_or_else(|| index.to_string(), str::to_owned)
            };
            let key = format!(
                "{}:response:{}",
                thread_id.unwrap_or_default(),
                source_id(first)
            );
            groups.push(ResponseGroup {
                source_keys: prose
                    .iter()
                    .map(|index| format!("{key}:source:{}", source_id(*index)))
                    .collect(),
                copy_text: prose
                    .iter()
                    .map(|index| codex::item_text(&items[*index]))
                    .collect::<Vec<_>>()
                    .join("\n\n"),
                received_at: items[*last]["groveReceivedAt"].as_u64(),
                key,
                end,
                prose,
            });
        }
        start = end + 1;
    }
    groups
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
        && index >= active_work_start(state)
        && state
            .saved
            .work_periods
            .iter()
            .rev()
            .find(|period| period.start_item <= index)
            .is_none_or(|period| period.end_item.is_none())
}

fn work_key_at(state: &codex::Snapshot, index: usize) -> String {
    let start = state.saved.items[..(index + 1).min(state.saved.items.len())]
        .iter()
        .rposition(|item| item["type"] == "userMessage")
        .unwrap_or(0);
    format!("work-{start}")
}

fn work_period_at(state: &codex::Snapshot, index: usize) -> Option<&codex::WorkPeriod> {
    state
        .saved
        .work_periods
        .iter()
        .rev()
        .find(|period| period.start_item <= index && period.end_item.is_none_or(|end| index < end))
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
    let period = work_period_at(state, index);
    let fallback_start = state.saved.items[..(index + 1).min(state.saved.items.len())]
        .iter()
        .rposition(|item| item["type"] == "userMessage")
        .unwrap_or(0);
    let start = period.map_or(fallback_start, |period| period.start_item);
    let active = work_is_active_at(state, index);
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
                format!("{verb} for {seconds}s")
            } else if minutes == 1 {
                format!("{verb} for {minutes}m {}s", seconds % 60)
            } else {
                format!("{verb} for {minutes}m")
            }
        },
    );
    Some((work_key_at(state, index), label, active))
}

/// Only parent commentary and tools belong to the work disclosure.
fn work_has_parent_activity(state: &codex::Snapshot, index: usize) -> bool {
    let start = work_period_at(state, index).map_or_else(
        || {
            state.saved.items[..index.min(state.saved.items.len())]
                .iter()
                .rposition(|item| item["type"] == "userMessage")
                .unwrap_or(0)
        },
        |period| period.start_item,
    );
    let end = work_period_at(state, index)
        .and_then(|period| period.end_item)
        .unwrap_or(state.saved.items.len());
    (start.min(state.saved.items.len())..end.min(state.saved.items.len())).any(|index| {
        let item = &state.saved.items[index];
        (is_tool_item(item) && !tasks::is_checklist(item)) || work_commentary(state, index)
    })
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

fn is_tool_item(item: &serde_json::Value) -> bool {
    !matches!(
        item["type"].as_str(),
        Some(
            "userMessage"
                | "agentMessage"
                | "plan"
                | "providerNotice"
                | "subAgentActivity"
                | codex::COMPACTION_ITEM
        )
    )
}

/// The agent's checklist, derived once per checklist change rather than per frame.
struct TaskCache {
    key: u64,
    list: Option<TaskList>,
}

/// Changes when a checklist snapshot changes or transcript indices change, so
/// streaming text reuses the cache.
fn task_cache_key(items: &[serde_json::Value]) -> u64 {
    let mut hasher = rustc_hash::FxHasher::default();
    items.len().hash(&mut hasher);
    for (index, item) in items.iter().enumerate() {
        if tasks::is_checklist(item) {
            index.hash(&mut hasher);
            hash_transcript_value(item, &mut hasher);
        }
    }
    hasher.finish()
}

fn task_cache(items: &[serde_json::Value], key: u64) -> TaskCache {
    TaskCache {
        key,
        list: tasks::current(items),
    }
}

/// How in-progress marks read for the session's state. Idle or finished
/// sessions keep a static mark while retaining their checklist.
fn task_phase(status: codex::Status) -> TaskPhase {
    match status {
        codex::Status::Starting | codex::Status::Working => TaskPhase::Running,
        codex::Status::Waiting => TaskPhase::Waiting,
        codex::Status::Interrupted | codex::Status::Failed | codex::Status::Disconnected => {
            TaskPhase::Halted
        }
        codex::Status::Idle | codex::Status::Done => TaskPhase::Stale,
    }
}

/// Completed marks stay green across session states and continuation turns.
fn task_mark_phase(status: TaskStatus, phase: TaskPhase) -> TaskPhase {
    if status == TaskStatus::Completed {
        TaskPhase::Running
    } else {
        phase
    }
}

/// The status whose mark leads the strip header.
fn task_header_status(list: &TaskList) -> TaskStatus {
    if list.all_done() {
        TaskStatus::Completed
    } else if list.current().is_some() {
        TaskStatus::InProgress
    } else {
        TaskStatus::Pending
    }
}

/// What the strip header says after its count and meter.
#[derive(Debug, PartialEq)]
enum TaskSummary {
    /// "All N tasks done", secondary.
    AllDone(usize),
    /// The step text, then "Waiting for you" in amber.
    Waiting(String),
    /// "Stopped at {step}", secondary.
    Stopped(String),
    /// The current task's activeForm, else its step.
    Now(String),
    None,
}

impl TaskSummary {
    fn text(&self) -> String {
        match self {
            Self::AllDone(total) => format!("All {total} tasks done"),
            Self::Waiting(step) => format!("{step} · Waiting for you"),
            Self::Stopped(step) => format!("Stopped at {step}"),
            Self::Now(label) => label.clone(),
            Self::None => String::new(),
        }
    }
}

fn task_summary(list: &TaskList, phase: TaskPhase) -> TaskSummary {
    if list.all_done() {
        return TaskSummary::AllDone(list.total());
    }
    let Some(current) = list.current() else {
        return TaskSummary::None;
    };
    match phase {
        TaskPhase::Waiting => TaskSummary::Waiting(current.step.clone()),
        TaskPhase::Halted => TaskSummary::Stopped(current.step.clone()),
        TaskPhase::Running | TaskPhase::Stale => {
            list.now_label().map_or(TaskSummary::None, |label| {
                TaskSummary::Now(label.to_owned())
            })
        }
    }
}

/// One task mark; the running spinner steps through its frames unless motion is reduced.
fn animated_task_mark(
    id: impl Into<gpui::ElementId>,
    status: TaskStatus,
    phase: TaskPhase,
    cx: &App,
) -> gpui::AnyElement {
    let slot = div()
        .size(rpx(ICON_SM))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center();
    if status == TaskStatus::InProgress && phase == TaskPhase::Running && !cx.reduce_motion() {
        slot.with_animation(
            id.into(),
            gpui::Animation::new(TASK_SPINNER_PERIOD).repeat(),
            move |slot, progress| {
                let frame =
                    (progress * crate::entities::animation_clock::SPINNER_FRAMES as f32) as u64;
                slot.child(task_mark(status, phase, frame * SPINNER_TICKS_PER_FRAME))
            },
        )
        .into_any_element()
    } else {
        slot.child(task_mark(status, phase, 0)).into_any_element()
    }
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
    if let Some(notice) = state
        .effective_notice()
        .filter(|notice| notice.kind == codex::SessionNoticeKind::Retrying)
    {
        return Some(format!("Retrying automatically · {}", notice.body));
    }
    let provider = state.saved.provider.label();
    if state.status == codex::Status::Starting && state.saved.provider == codex::Provider::Claude {
        return Some(format!("Connecting to {provider}…"));
    }
    if compacting(state) {
        return Some("Compacting context…".into());
    }
    if !state.status.busy()
        || (state.saved.items.last().is_some_and(is_tool_item)
            && state.questions.is_empty()
            && state.elicitations.is_empty()
            && state.approvals.is_empty())
    {
        return None;
    }
    // The active work disclosure already provides the turn's liveness signal.
    // Keep the initial fallback when no work disclosure has mounted yet.
    let current_start = state
        .saved
        .work_periods
        .last()
        .filter(|period| period.end_item.is_none())
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
    let header_index = (current_start..state.saved.items.len())
        .find(|index| state.saved.items[*index]["type"] != "userMessage")
        .unwrap_or(state.saved.items.len());
    let has_work_header = work_header_at(state, header_index).is_some_and(|(_, _, active)| active);
    if state.questions.is_empty()
        && state.elicitations.is_empty()
        && state.approvals.is_empty()
        && has_work_header
        && !(state.saved.items.is_empty() && state.saved.thread_id.is_none())
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

fn tool_failed(item: &serde_json::Value) -> bool {
    item["status"] == "failed"
        || item["is_error"] == true
        || item["exitCode"].as_i64().is_some_and(|code| code != 0)
        || item.get("error").is_some_and(|error| !error.is_null())
}

/// Quiet activity rows describe the invocation; output stays behind disclosure.
fn tool_activity_summary(item: &serde_json::Value, running: bool) -> String {
    if item["type"] == "reasoning" {
        return "Thinking…".into();
    }
    if item["type"] == "commandExecution" {
        let command = item["command"].as_str().unwrap_or("command");
        return format!(
            "{} {}",
            if running {
                "Running"
            } else if tool_failed(item) {
                "Command failed ·"
            } else {
                "Ran"
            },
            command.split_whitespace().collect::<Vec<_>>().join(" ")
        ) + if running { "…" } else { "" };
    }
    // Waiting tools keep one label after completion; their raw identity stays
    // in the disclosure, like every other tool's arguments and output.
    if item["tool"].as_str().is_some_and(|tool| {
        matches!(
            tool.rsplit("__")
                .next()
                .unwrap_or(tool)
                .to_ascii_lowercase()
                .as_str(),
            "wait" | "wait_agent"
        )
    }) {
        return "Waiting…".into();
    }
    if !running {
        return if tool_failed(item) {
            format!("Tool failed · {}", tool_summary(item))
        } else {
            tool_summary(item)
        };
    }
    match item["type"].as_str().unwrap_or_default() {
        "fileChange" => "Updating files…".into(),
        "webSearch" => "Searching the web…".into(),
        _ => {
            let tool = item["tool"].as_str().unwrap_or_default();
            // Namespaced MCP tools retain their full identity in the disclosure.
            let action = tool
                .rsplit("__")
                .next()
                .unwrap_or(tool)
                .to_ascii_lowercase();
            match action.as_str() {
                "sleep" => "Sleeping…".into(),
                "wait" | "wait_agent" => "Waiting…".into(),
                "think" | "thought" | "thinking" => "Thinking…".into(),
                "read" => "Reading…".into(),
                "write" => "Writing…".into(),
                "edit" | "multiedit" => "Editing…".into(),
                "search" | "grep" | "glob" | "websearch" => "Searching…".into(),
                "fetch" | "webfetch" => "Fetching…".into(),
                _ if !tool.is_empty() => format!("Using {tool}…"),
                _ => "Working…".into(),
            }
        }
    }
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

fn active_turn_redirect_available(state: &codex::Snapshot) -> bool {
    matches!(
        state.status,
        codex::Status::Working | codex::Status::Waiting
    ) && state.turn_id.is_some()
        && !state.compacting
        && state.saved.pending_edit.is_none()
        && (state.saved.provider == codex::Provider::Codex || state.can_redirect)
}

const STEER_LABEL: &str = "Steer";
const STEER_DESCRIPTION: &str = "Steer into the current turn";

fn tool_detail(item: &serde_json::Value) -> String {
    let mut text = if item["type"] == "plan" && item["tool"].is_string() {
        String::new()
    } else {
        codex::item_text(item)
    };
    if matches!(
        item["type"].as_str(),
        Some("fileChange" | "webSearch" | "plan")
    ) {
        if let Some(content) = item["result"]["content"].as_array() {
            for result in content.iter().filter_map(|block| block["text"].as_str()) {
                if !result.is_empty() && !text.contains(result) {
                    text.push_str("\n\n");
                    text.push_str(result);
                }
            }
        }
        if let Some(error) = item["error"]
            .as_str()
            .or_else(|| item["error"]["message"].as_str())
        {
            if !text.contains(error) {
                text.push_str("\n\n");
                text.push_str(error);
            }
        }
        if let Some(arguments) = item.get("arguments") {
            return format!(
                "Arguments:\n{}\n\n{}",
                serde_json::to_string_pretty(arguments).unwrap_or_default(),
                text.trim()
            );
        }
    }
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
        "commandExecution" | "fileChange" | "reasoning" | "webSearch" | "plan" => text,
        _ => serde_json::to_string_pretty(item).unwrap_or(text),
    }
}

pub(super) fn markdown_style(window: &Window) -> TextViewStyle {
    let mut style = TextViewStyle::default();
    style.is_dark = c::is_dark();
    style.heading_base_font_size = rpx(TEXT_BODY).to_pixels(window.rem_size());
    style.highlight_theme = c::highlight_theme();
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

/// Decode only complete canonical reference prompts. All other user text stays literal.
fn reference_prompt(text: &str) -> Option<(codex::ReferenceDraft, &str)> {
    let body = text.strip_prefix("Referenced transcript selection (quoted data):\n")?;
    let (json, reply) = body.split_once("\n\nUser reply:\n")?;
    let reference: codex::ReferenceDraft = serde_json::from_str(json).ok()?;
    if reference.id.is_empty() || reference.sources.is_empty() || reference.serialize(reply) != text
    {
        return None;
    }
    Some((reference, reply))
}

fn reference_prompt_copy(text: &str) -> String {
    let Some((reference, reply)) = reference_prompt(text) else {
        return text.to_owned();
    };
    let roles = reference
        .sources
        .iter()
        .map(|source| source.role.as_str())
        .collect::<Vec<_>>()
        .join(" · ");
    format!(
        "Referenced selection · {roles}\n{}\n\n{}",
        reference.text, reply
    )
}

fn reference_prompt_markdown(text: &str) -> String {
    let Some((reference, reply)) = reference_prompt(text) else {
        return text.to_owned();
    };
    // A fence longer than any run in the literal passage prevents selected
    // Markdown/code from becoming authored transcript chrome.
    let longest = reference
        .text
        .split(|ch| ch != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    let roles = reference
        .sources
        .iter()
        .map(|source| source.role.as_str())
        .collect::<Vec<_>>()
        .join(" · ");
    format!(
        "Referenced selection · {roles}\n\n{fence}text\n{}\n{fence}\n\n{}",
        reference.text, reply
    )
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
    let start = prefix
        .char_indices()
        .rfind(|(_, ch)| ch.is_whitespace())
        .map_or(0, |(at, ch)| at + ch.len_utf8());
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
    fn elicitation_key(request: &ElicitationRequest, epoch: u64) -> String {
        format!("{epoch}:{request:?}")
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
            .map(|request| Self::elicitation_key(request, state.interaction_epoch))
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
            let key = Self::elicitation_key(request, state.interaction_epoch);
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
    fn elicitation_field_value(
        field: &grove_core::elicitation::ElicitationField,
        draft: &ElicitationFieldDraft,
        cx: &App,
    ) -> Result<serde_json::Value, String> {
        let text = draft.input.read(cx).value().to_string();
        let value = match &field.kind {
            ElicitationKind::String { .. } => serde_json::Value::String(text),
            ElicitationKind::Number { integer, .. } => {
                let number = serde_json::from_str::<serde_json::Value>(text.trim())
                    .ok()
                    .filter(|v| {
                        v.is_number() && (!integer || v.as_i64().is_some() || v.as_u64().is_some())
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
        Ok(value)
    }
    fn elicitation_field_error(
        field: &grove_core::elicitation::ElicitationField,
        draft: &ElicitationFieldDraft,
        cx: &App,
    ) -> Option<String> {
        let mut content = serde_json::Map::new();
        if draft.included {
            match Self::elicitation_field_value(field, draft, cx) {
                Ok(value) => {
                    content.insert(field.name.clone(), value);
                }
                Err(error) => return Some(error),
            }
        }
        grove_core::elicitation::ElicitationForm {
            fields: vec![field.clone()],
        }
        .validate(&content)
        .err()
        .and_then(|errors| errors.into_iter().next())
        .map(|error| error.message)
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
            let value = Self::elicitation_field_value(field, draft, cx)?;
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
            .find(|request| {
                request.id == id
                    && Self::elicitation_key(request, self.session.read(cx).state.interaction_epoch)
                        == key
            })
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
        let epoch = self.session.read(cx).state.interaction_epoch;
        self.session.update(cx, |session, cx| {
            session.send(
                Action::Interaction {
                    epoch,
                    action: Box::new(Action::RespondElicitation {
                        id,
                        action: action.to_owned(),
                        content,
                    }),
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
            let key = Self::elicitation_key(request, state.interaction_epoch);
            let draft = &self.elicitation_drafts[&key];
            let source = state.interaction_agent(&request.id).map_or_else(
                || request.server_name.clone(),
                |agent| format!("{agent} · {}", request.server_name),
            );
            let mut group = div()
                .id(("agent-elicitation", request_index))
                .p(rpx(SPACE_2XL))
                .rounded(rpx(RADIUS_PANEL))
                .border_1()
                .border_color(c::BORDER_SOFT())
                .bg(c::BG_RAIL())
                .role(gpui::Role::Group)
                .aria_label(format!("Input requested by {source}"))
                .w_full()
                .min_w_0()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .child(permission_heading(
                    format!("{} needs {}", if request.server_name == "cua_repl" { "Computer Use" } else { &request.server_name }, if matches!(&request.mode, ElicitationMode::Form(form) if form.fields.is_empty()) { "permission" } else { "your input" }),
                    format!("{} · {source}", state.saved.provider.label()),
                ))
                .child(literal_notice_text(format!("elicitation-message-{request_index}"), &request.message));
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
                            .icon(
                                gpui_component::Icon::default()
                                    .path("icons/external-link.svg")
                                    .size(rpx(ICON_SM)),
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
                        let field_error = Self::elicitation_field_error(field, field_draft, cx);
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
                                            field_error.is_some(),
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
                        group = group.child(attached_validation(
                            gpui::SharedString::from(format!(
                                "elicitation-validation-field-{request_index}-{index}"
                            )),
                            row,
                            field_error.as_deref(),
                        ));
                    }
                    if let Err(error) = &valid {
                        group = group.child(
                            div()
                                .id(gpui::SharedString::from(format!(
                                    "elicitation-validation-{request_index}"
                                )))
                                .role(gpui::Role::Alert)
                                .text_size(rpx(TEXT_SMALL))
                                .child(status::label(
                                    format!("elicitation-error-status-{request_index}"),
                                    StatusKind::Failed,
                                    error.clone(),
                                    ICON_SM,
                                    cx,
                                )),
                        );
                    }
                }
            }
            let mut buttons = div().flex().flex_wrap().gap(rpx(SPACE_LG));
            for (action, label) in [
                (
                    "accept",
                    if matches!(request.mode, ElicitationMode::Url { .. }) {
                        "I've completed this"
                    } else if matches!(&request.mode, ElicitationMode::Form(form) if form.fields.is_empty())
                    {
                        "Allow"
                    } else {
                        "Submit"
                    },
                ),
                ("decline", "Decline"),
                ("cancel", "Cancel request"),
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
                    .icon(
                        gpui_component::Icon::default()
                            .path(gpui::SharedString::from(format!(
                                "icons/{}.svg",
                                if action == "accept" { "check" } else { "close" }
                            )))
                            .size(rpx(ICON_SM)),
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
            .find(|request| {
                Self::elicitation_key(request, self.session.read(cx).state.interaction_epoch) == key
            })
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
        .when(submitting, |row| {
            row.cursor_default().opacity(OPACITY_DISABLED)
        })
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
        let keys: HashSet<_> = state
            .approvals
            .iter()
            .map(|approval| Self::approval_key(approval, state.interaction_epoch))
            .collect();
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
            let key = Self::approval_key(approval, state.interaction_epoch);
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
                    policy_expanded: false,
                    submitting: false,
                    _subscriptions: subscriptions,
                },
            );
        }
    }
    fn approval_key(approval: &serde_json::Value, epoch: u64) -> String {
        format!("{epoch}:{approval}")
    }
    fn respond_approval(
        &mut self,
        key: &str,
        id: serde_json::Value,
        choice: String,
        cx: &mut Context<Self>,
    ) {
        let state = &self.session.read(cx).state;
        let Some(request) = state.approvals.iter().find(|request| {
            Self::approval_key(request, state.interaction_epoch) == key && request["id"] == id
        }) else {
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
        let epoch = state.interaction_epoch;
        self.session.update(cx, |session, cx| {
            session.send(
                Action::Interaction {
                    epoch,
                    action: Box::new(Action::RespondApproval {
                        id,
                        choice,
                        input,
                        feedback,
                    }),
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
            let key = Self::approval_key(approval, state.interaction_epoch);
            let draft = &self.approval_drafts[&key];
            let mut group = div()
                .id(("codex-approval", index))
                .p(rpx(SPACE_2XL))
                .rounded(rpx(RADIUS_PANEL))
                .border_1()
                .border_color(c::BORDER_SOFT())
                .bg(c::BG_RAIL())
                .role(gpui::Role::Group)
                .aria_label(grove_core::controls::approval_title(approval))
                .w_full()
                .min_w_0()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .font_family(crate::fonts::UI_FAMILY)
                .font_weight(gpui::FontWeight::NORMAL)
                .text_size(rpx(TEXT_SMALL))
                .text_color(c::FG())
                .child(permission_heading(
                    grove_core::controls::approval_title(approval),
                    state.saved.provider.label().into(),
                ));
            if let Some(command) = grove_core::controls::approval_command(approval) {
                if let Some(reason) = approval["params"]["reason"]
                    .as_str()
                    .or_else(|| approval["claudeInput"]["description"].as_str())
                {
                    group = group.child(literal_notice_text(
                        format!("approval-reason-{index}"),
                        reason,
                    ));
                }
                group = group.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .p(rpx(SPACE_LG))
                        .rounded(rpx(RADIUS_CONTROL))
                        .bg(c::BG())
                        .child(permission_code(
                            format!("approval-command-{index}"),
                            command,
                        )),
                );
                if let Some(cwd) = approval["params"]["cwd"].as_str() {
                    group = group
                        .child(div().text_color(c::FG_DIM()).child("Working directory"))
                        .child(literal_notice_text(format!("approval-cwd-{index}"), cwd));
                }
                let extra = grove_core::controls::approval_command_extra(approval);
                if !extra.is_empty() {
                    group = group.child(literal_notice_text(
                        format!("approval-extra-{index}"),
                        &extra,
                    ));
                }
            } else {
                group = group.child(literal_notice_text(
                    format!("approval-details-{index}"),
                    &grove_core::controls::approval_details(approval),
                ));
            }
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
                    .when(draft.submitting, |row| {
                        row.cursor_default().opacity(OPACITY_DISABLED)
                    })
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
                                .child(status::label(
                                    format!("approval-error-status-{index}"),
                                    StatusKind::Failed,
                                    "Enter a valid JSON object before approving.",
                                    ICON_SM,
                                    cx,
                                )),
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
            let mut policy_button = None;
            for (choice_index, choice) in grove_core::controls::approval_choices(approval)
                .into_iter()
                .enumerate()
            {
                let click_key = key.clone();
                let id = approval["id"].clone();
                let denying = matches!(choice.id.as_str(), "deny" | "cancel");
                let is_policy = choice.id == "execpolicy";
                let button = transcript_action(
                    gpui::SharedString::from(format!("approval-{index}-{choice_index}")),
                    choice.label,
                    choice.id == "allow_once",
                    window,
                    cx,
                )
                .icon(
                    gpui_component::Icon::default()
                        .path(gpui::SharedString::from(format!(
                            "icons/{}.svg",
                            if denying { "close" } else { "check" }
                        )))
                        .size(rpx(ICON_SM)),
                )
                .debug_selector(move || format!("approval-{index}-{choice_index}"))
                .disabled(draft.submitting || (!denying && !valid_json))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.respond_approval(&click_key, id.clone(), choice.id.clone(), cx);
                }));
                if is_policy {
                    policy_button = Some(button);
                } else {
                    buttons = buttons.child(button);
                }
            }
            group = group.child(buttons);
            if let Some(button) = policy_button {
                let click_key = key.clone();
                group = group.child(
                    div()
                        .flex()
                        .flex_col()
                        .items_start()
                        .border_t_1()
                        .border_color(c::BORDER_SOFT())
                        .pt(rpx(SPACE_LG))
                        .child(
                            transcript_action(
                                gpui::SharedString::from(format!("approval-policy-{index}")),
                                "Matching command prefix",
                                false,
                                window,
                                cx,
                            )
                            .icon(
                                gpui_component::Icon::default()
                                    .path(if draft.policy_expanded {
                                        "icons/chev-down.svg"
                                    } else {
                                        "icons/chev-right.svg"
                                    })
                                    .size(rpx(ICON_SM)),
                            )
                            .aria_expanded(draft.policy_expanded)
                            .debug_selector(move || format!("approval-policy-{index}"))
                            .disabled(draft.submitting)
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.toggle_approval_policy(&click_key, cx),
                            )),
                        ),
                );
                if draft.policy_expanded {
                    group = group
                        .child(
                            div()
                                .text_color(c::FG_DIM())
                                .child("Proposed execution policy amendment"),
                        )
                        .child(permission_code(
                            format!("approval-prefix-{index}"),
                            &serde_json::to_string_pretty(
                                &approval["params"]["proposedExecpolicyAmendment"],
                            )
                            .unwrap_or_default(),
                        ))
                        .child(button);
                }
            }
            panel = panel.child(group);
        }
        panel
    }
    fn toggle_approval_policy(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(draft) = self.approval_drafts.get_mut(key) {
            if !draft.submitting {
                draft.policy_expanded = !draft.policy_expanded;
                cx.notify();
            }
        }
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
                            "{}:{}:{}:{}",
                            state.interaction_epoch,
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
        // Async answers become ordinary user messages. Keep the acknowledgement
        // only until that message arrives; tool answers still need their receipt.
        self.question_receipts.retain(|receipt| {
            !receipt.echo_text.as_ref().is_some_and(|echo| {
                state
                    .saved
                    .items
                    .iter()
                    .skip(receipt.echo_after)
                    .any(|item| item["type"] == "userMessage" && codex::item_text(item) == *echo)
            })
        });
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
            echo_text: request.answer_echo_text(answers),
            echo_after: state.saved.items.len(),
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
        let epoch = state.interaction_epoch;
        self.session.update(cx, |session, cx| {
            session.state.clear_error();
            session.send(
                Action::Interaction {
                    epoch,
                    action: Box::new(Action::AnswerQuestions(id, answers)),
                },
                cx,
            );
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
            .aria_label(format!(
                "Questions from {}",
                state
                    .interaction_agent(&request.id)
                    .unwrap_or(state.saved.provider.label())
            ))
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
                    .text_color(c::FG())
                    .child(format!(
                        "{} needs your input · {answered} of {} answered · draft",
                        state
                            .interaction_agent(&request.id)
                            .unwrap_or(state.saved.provider.label()),
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
                .when(submitting, gpui::Styled::cursor_default)
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
                    .when(submitting, |row| {
                        row.cursor_default().opacity(OPACITY_DISABLED)
                    })
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
                    .when(submitting, |row| {
                        row.cursor_default().opacity(OPACITY_DISABLED)
                    })
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
                let mut single = request.clone();
                single.questions = vec![question.clone()];
                let own_answers = answers
                    .iter()
                    .filter(|(id, _)| **id == question.id)
                    .map(|(id, values)| (id.clone(), values.clone()))
                    .collect();
                let field_error = single.validate(&own_answers).err();
                let focused = question_draft.input.focus_handle(cx).is_focused(window);
                field = field.child(attached_validation(
                    gpui::SharedString::from(format!(
                        "question-validation-field-{request_index}-{question_index}"
                    )),
                    project_field_well(focused, field_error.is_some())
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
                    field_error.as_deref(),
                ));
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
                    .child(status::label(
                        format!("question-error-status-{request_index}"),
                        StatusKind::Failed,
                        error,
                        ICON_SM,
                        cx,
                    )),
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
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/close.svg")
                            .size(rpx(ICON_SM)),
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
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/check.svg")
                            .size(rpx(ICON_SM)),
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
    #[test]
    fn composer_markdown_styles_nested_unicode_source_without_overlapping_ranges() {
        let source =
            "🦀 **bold *é*** and ~~gone~~ and `code`\n\n# Heading\n\n[site](https://example.com)";
        let decorations = super::composer_markdown_decorations(source);
        let at = |text: &str| {
            let offset = source.find(text).unwrap();
            decorations
                .iter()
                .find(|d| d.range.contains(&offset))
                .unwrap()
                .style
        };
        assert_eq!(at("bold").font_weight, Some(gpui::FontWeight::BOLD));
        assert_eq!(at("é").font_weight, Some(gpui::FontWeight::BOLD));
        assert_eq!(at("é").font_style, Some(gpui::FontStyle::Italic));
        assert!(at("gone").strikethrough.is_some());
        assert!(at("code").background_color.is_some());
        assert_eq!(at("Heading").font_weight, Some(gpui::FontWeight::BOLD));
        assert!(at("site").underline.is_some());
        for pair in decorations.windows(2) {
            assert!(pair[0].range.end <= pair[1].range.start);
        }
        for decoration in decorations {
            assert!(source.get(decoration.range).is_some());
        }
        assert!(super::composer_markdown_decorations("**unfinished").is_empty());
    }

    #[test]
    fn composer_markdown_projects_headings_lists_and_inline_syntax() {
        let source = "## Heading 2\n- A **bold** item with `code` and [link](https://example.com)\n1. Ordered 🦀";
        let replacements = super::composer_markdown_replacements(source);
        let mut rendered = String::new();
        let mut previous = 0;
        for replacement in &replacements {
            assert!(replacement.range.start >= previous);
            rendered.push_str(&source[previous..replacement.range.start]);
            rendered.push_str(&replacement.text);
            previous = replacement.range.end;
        }
        rendered.push_str(&source[previous..]);
        assert_eq!(
            rendered,
            "Heading 2\n• A bold item with code and link\n1. Ordered 🦀"
        );
        assert!(replacements.iter().any(|r| r.font_scale > 1.));
        assert!(super::composer_markdown_replacements("**unfinished").is_empty());
    }

    #[gpui::test]
    fn composer_markdown_preserves_source_navigation_copy_and_ime_for_both_providers(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::EntityInputHandler as _;
        let (view, cx, _, _) = fixture(cx);
        let source = "## Heading 2\n- **é🦀** item\n1. Ordered";
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.session
                        .update(cx, |session, _| session.state.saved.provider = provider);
                    view.input.update(cx, |input, cx| {
                        input.set_value(source, window, cx);
                        input.set_selected_range(3..3, cx);
                        input.focus(window, cx);
                    });
                });
            });
            draw(cx);
            let bounds = cx.debug_bounds("codex-composer-input").unwrap();
            let (heading, item) = cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.input.update(cx, |input, cx| {
                        let heading = input.bounds_for_range(3..3, bounds, window, cx).unwrap();
                        let item_start = source.find('é').unwrap();
                        let utf16 = source[..item_start].encode_utf16().count();
                        let item = input
                            .bounds_for_range(utf16..utf16, bounds, window, cx)
                            .unwrap();
                        assert!(
                            item.top() > heading.top(),
                            "projected source offsets retain separate rows"
                        );
                        (heading, item)
                    })
                })
            });
            cx.simulate_click(
                heading.origin + gpui::point(gpui::px(0.1), heading.size.height / 2.),
                gpui::Modifiers::default(),
            );
            draw(cx);
            view.read_with(cx, |view, cx| {
                assert_eq!(
                    view.input.read(cx).cursor(),
                    3,
                    "clicking rendered heading begins after source hashes"
                );
            });
            cx.simulate_click(
                item.origin + gpui::point(gpui::px(0.1), item.size.height / 2.),
                gpui::Modifiers::default(),
            );
            draw(cx);
            view.read_with(cx, |view, cx| {
                assert_eq!(
                    view.input.read(cx).cursor(),
                    source.find('é').unwrap(),
                    "clicking Unicode list content maps through hidden strong and bullet syntax"
                );
            });
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.input
                        .update(cx, |input, cx| input.set_selected_range(3..3, cx));
                });
            });
            draw(cx);
            cx.simulate_keystrokes("left");
            draw(cx);
            view.read_with(cx, |view, cx| assert_eq!(view.input.read(cx).cursor(), 2));
            cx.simulate_keystrokes("right");
            draw(cx);
            view.read_with(cx, |view, cx| assert_eq!(view.input.read(cx).cursor(), 3));
            cx.simulate_keystrokes(if cfg!(target_os = "macos") {
                "cmd-a cmd-c"
            } else {
                "ctrl-a ctrl-c"
            });
            cx.update(|_, cx| {
                assert_eq!(
                    cx.read_from_clipboard().unwrap().text().as_deref(),
                    Some(source)
                );
            });
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.input.update(cx, |input, cx| {
                        input.set_selected_range(source.len()..source.len(), cx);
                        input.replace_and_mark_text_in_range(None, "日本", Some(2..2), window, cx);
                    });
                });
            });
            draw(cx);
            view.read_with(cx, |view, cx| {
                assert_eq!(
                    view.input.read(cx).value().as_ref(),
                    format!("{source}日本")
                );
                assert_eq!(view.input.read(cx).cursor(), source.len() + "日本".len());
            });
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.input.update(cx, |input, cx| {
                        input.replace_text_in_range(None, "日本語", window, cx);
                    });
                });
            });
            draw(cx);
            view.read_with(cx, |view, cx| {
                assert_eq!(
                    view.input.read(cx).value().as_ref(),
                    format!("{source}日本語")
                );
            });
            cx.simulate_keystrokes(if cfg!(target_os = "macos") {
                "cmd-z"
            } else {
                "ctrl-z"
            });
            draw(cx);
            view.read_with(cx, |view, cx| {
                assert_eq!(
                    view.input.read(cx).value().as_ref(),
                    source,
                    "presentation does not add undo edits"
                );
            });
            cx.simulate_keystrokes(if cfg!(target_os = "macos") {
                "cmd-shift-z"
            } else {
                "ctrl-y"
            });
            draw(cx);
            view.read_with(cx, |view, cx| {
                assert_eq!(
                    view.input.read(cx).value().as_ref(),
                    format!("{source}日本語")
                );
            });
        }
    }

    #[gpui::test]
    fn composer_markdown_keeps_literal_draft_caret_and_picked_tokens(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        let source = "🦀 **bold** @src/main.rs\n`code`";
        let range = source.find('@').unwrap()..source.find('\n').unwrap();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value(source, window, cx);
                    input.set_selected_range(4..4, cx);
                });
                let decoration = view.input.update(cx, |input, cx| {
                    input.create_decorations_collection(
                        vec![TextDecoration::new(
                            range.clone(),
                            gpui::HighlightStyle {
                                underline: Some(gpui::UnderlineStyle::default()),
                                ..Default::default()
                            },
                        )],
                        cx,
                    )
                });
                view.picked_tokens.push(ComposerToken {
                    value: "@src/main.rs".into(),
                    decoration,
                });
            });
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.input.read(cx).value().as_ref(), source);
            assert_eq!(view.input.read(cx).cursor(), 4);
            assert!(!view.markdown_decorations.get_ranges(cx).is_empty());
            assert_eq!(view.picked_tokens[0].decoration.get_ranges(cx), vec![range]);
        });
    }

    #[gpui::test]
    fn unchanged_composer_decorations_do_not_notify_input(cx: &mut gpui::TestAppContext) {
        let (view, cx, _, _) = fixture(cx);
        let notifications = Rc::new(Cell::new(0));
        let count = notifications.clone();
        let span = TextDecoration::new(
            2..6,
            gpui::HighlightStyle {
                font_weight: Some(gpui::FontWeight::BOLD),
                ..Default::default()
            },
        );
        let (input, decorations, _observer) = cx.update(|window, cx| {
            view.update(cx, |_, cx| {
                let input = cx.new(|cx| InputState::new(window, cx).default_value("**bold**"));
                let decorations = input.update(cx, |input, cx| {
                    input.create_decorations_collection(vec![span.clone()], cx)
                });
                let observer = cx.observe(&input, move |_, _, _| count.set(count.get() + 1));
                (input, decorations, observer)
            })
        });
        cx.run_until_parked();
        let baseline = notifications.get();
        cx.update(|_, cx| decorations.set(vec![span.clone()], cx));
        cx.run_until_parked();
        assert_eq!(
            notifications.get(),
            baseline,
            "identical spans must not schedule another render"
        );
        cx.update(|_, cx| decorations.set(vec![TextDecoration::new(2..5, span.style)], cx));
        cx.run_until_parked();
        assert_eq!(
            notifications.get(),
            baseline + 1,
            "changed spans must notify"
        );
        cx.update(|window, cx| {
            input.update(cx, |input, cx| input.set_value("**bold**", window, cx));
        });
        cx.run_until_parked();
        let after_edit = notifications.get();
        cx.update(|_, cx| decorations.set(vec![span.clone()], cx));
        cx.run_until_parked();
        assert_eq!(
            notifications.get(),
            after_edit + 1,
            "span restoration after replacement must notify"
        );
    }

    #[test]
    fn shell_execution_requires_a_closed_fence() {
        assert!(super::closed_shell_fence("```sh\necho hi\n```"));
        assert!(super::closed_shell_fence("~~~~bash\nprintf x\n~~~~"));
        assert!(!super::closed_shell_fence("```sh\necho hi"));
        assert!(!super::closed_shell_fence("```sh\necho hi\n``"));
        assert!(!super::closed_shell_fence("echo hi"));
    }

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
    fn transcript_and_composer_edges_match_with_navigation_at_each_width_and_zoom(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            for has_navigation in [false, true] {
                cx.update(|_, cx| {
                    cx.set_reduce_motion(true);
                    view.update(cx, |view, cx| {
                        view.session.update(cx, |session, cx| {
                            session.state.saved.provider = provider;
                            session.state.saved.items = if has_navigation {
                                vec![
                                    json!({"id":"align-user", "type":"userMessage", "content":[{"text":"Check alignment"}]}),
                                    json!({"id":"align-agent", "type":"agentMessage", "text":"Aligned response"}),
                                ]
                            } else {
                                vec![json!({"id":"align-agent", "type":"agentMessage", "text":"Aligned response"})]
                            };
                            cx.notify();
                        });
                    });
                });
                for (width, rem) in [(1200.0, 16.0), (600.0, 16.0), (400.0, 16.0), (400.0, 20.0)] {
                    cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.0)));
                    cx.update(|_, cx| {
                        gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                    });
                    draw(cx);
                    let anchor = cx
                        .debug_bounds(if has_navigation {
                            "codex-message-anchor-1"
                        } else {
                            "codex-message-anchor-0"
                        })
                        .unwrap();
                    let composer = cx.debug_bounds("codex-composer").unwrap();
                    assert!(
                        (anchor.left() - composer.left()).abs() <= gpui::px(1.0),
                        "left edge: {provider:?}, navigation={has_navigation}, width={width}, rem={rem}: {anchor:?} vs {composer:?}"
                    );
                    assert!(
                        (anchor.right() - composer.right()).abs() <= gpui::px(1.0),
                        "right edge: {provider:?}, navigation={has_navigation}, width={width}, rem={rem}: {anchor:?} vs {composer:?}"
                    );
                }
            }
        }
    }

    #[gpui::test]
    fn minimap_stays_anchored_when_tasks_and_composer_expand(cx: &mut gpui::TestAppContext) {
        let (view, cx, _, _) = fixture(cx);
        cx.update(|_, cx| {
            cx.set_reduce_motion(true);
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.items = vec![
                        json!({"id":"u1", "type":"userMessage", "content":[{"text":"First input"}]}),
                        json!({"id":"a1", "type":"agentMessage", "text":"First response"}),
                        json!({"id":"u2", "type":"userMessage", "content":[{"text":"Second input"}]}),
                        json!({"id":"tasks", "type":"plan", "plan":[
                            {"step":"Inspect", "status":"completed"},
                            {"step":"Implement", "status":"pending"},
                            {"step":"Verify", "status":"pending"}]}),
                    ];
                    cx.notify();
                });
            });
        });
        for width in [1000.0, 400.0] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.0)));
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.tasks_expanded = false;
                    view.composer_collapsed = true;
                    view.input
                        .update(cx, |input, cx| input.set_value("", window, cx));
                    cx.notify();
                });
            });
            draw(cx);
            let rail = cx.debug_bounds("codex-message-minimap").unwrap();
            let marker = cx.debug_bounds("codex-minimap-message-0").unwrap();
            let transcript = cx.debug_bounds("codex-messages").unwrap();
            let composer = cx.debug_bounds("codex-composer").unwrap();
            cx.update(|_, cx| view.update(cx, CodexView::toggle_tasks));
            draw(cx);
            assert!(
                cx.debug_bounds("codex-messages").unwrap().size.height < transcript.size.height
            );
            assert_eq!(cx.debug_bounds("codex-message-minimap").unwrap(), rail);
            assert_eq!(cx.debug_bounds("codex-minimap-message-0").unwrap(), marker);
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.composer_collapsed = false;
                    view.input.update(cx, |input, cx| {
                        input.set_value("A longer draft\n".repeat(20), window, cx);
                    });
                    cx.notify();
                });
            });
            draw(cx);
            assert!(cx.debug_bounds("codex-composer").unwrap().size.height > composer.size.height);
            assert_eq!(cx.debug_bounds("codex-message-minimap").unwrap(), rail);
            assert_eq!(cx.debug_bounds("codex-minimap-message-0").unwrap(), marker);
        }
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
    fn child_preview_wheel_browsing_survives_updates_for_both_providers(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            cx.update(|_, cx| {
                cx.set_reduce_motion(true);
                view.update(cx, |view, cx| {
                    view.preview_child = Some("child".into());
                    view.preview_follow_latest = true;
                    view.session.update(cx, |session, cx| {
                        session.state.saved.provider = provider;
                        session.state.status = Status::Working;
                        let mut activity = codex::SubagentActivity::default();
                        activity.observed_at = Some(std::time::Instant::now());
                        activity.items = (0..32).map(|index| json!({
                            "id": format!("child-{index}"), "type": "agentMessage",
                            "text": format!("Step {index}\n\nDetailed child response.\n\nMore details.")
                        })).collect();
                        session.state.subagent_activity.insert("child".into(), activity);
                        cx.notify();
                    });
                });
            });
            draw(cx);
            draw(cx);
            view.read_with(cx, |view, _| {
                assert_eq!(view.scroll.offset().y, -view.scroll.max_offset().y);
                assert!(view.scroll.max_offset().y > gpui::px(500.));
            });
            let wheel = |delta, cx: &mut gpui::VisualTestContext| {
                let bounds = cx.debug_bounds("codex-messages").unwrap();
                cx.simulate_event(gpui::ScrollWheelEvent {
                    position: bounds.center(),
                    delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(delta))),
                    modifiers: gpui::Modifiers::default(),
                    touch_phase: gpui::TouchPhase::Moved,
                });
                draw(cx);
                draw(cx);
            };
            wheel(300., cx);
            let browsing = view.read_with(cx, |view, _| {
                assert!(
                    view.scroll.offset().y > -view.scroll.max_offset().y + gpui::px(200.),
                    "{provider:?}: wheel must reveal older content"
                );
                assert!(!view.composer_collapsed);
                view.scroll.offset().y
            });
            cx.executor()
                .advance_clock(std::time::Duration::from_secs(2));
            cx.update(|_, cx| view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.subagent_activity.get_mut("child").unwrap().items.push(json!({
                        "id": "incoming", "type": "agentMessage", "text": "Incoming child update.\n\nMore streamed content."
                    }));
                    cx.notify();
                });
            }));
            draw(cx);
            draw(cx);
            view.read_with(cx, |view, _| {
                assert_eq!(
                    view.scroll.offset().y,
                    browsing,
                    "{provider:?}: updates must preserve browsing"
                );
            });
            wheel(-100000., cx);
            view.read_with(cx, |view, _| {
                assert!(
                    view.preview_follow_latest,
                    "{provider:?}: returning to bottom resumes follow"
                );
                assert_eq!(view.scroll.offset().y, -view.scroll.max_offset().y);
            });
            cx.update(|_, cx| view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.subagent_activity.get_mut("child").unwrap().items.push(json!({
                        "id": "latest", "type": "agentMessage", "text": "Latest response.\n\nFollowing again."
                    }));
                    cx.notify();
                });
            }));
            draw(cx);
            draw(cx);
            view.read_with(cx, |view, _| {
                assert_eq!(
                    view.scroll.offset().y,
                    -view.scroll.max_offset().y,
                    "{provider:?}: new items follow after returning to bottom"
                );
            });
        }
    }

    #[gpui::test]
    fn child_preview_uses_transcript_tools_and_follows_incoming_items(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        cx.update(|_, cx| view.update(cx, |view, cx| {
            view.preview_child = Some("child".into());
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Working;
                let mut activity = codex::SubagentActivity::default();
                activity.observed_at = Some(std::time::Instant::now());
                activity.items = (0..24).map(|index| json!({"id":format!("child-{index}"),"type":"agentMessage","text":format!("## Step {index}\n\nDetailed investigation with **markdown** and `code`.\n\nAnother paragraph to exercise transcript scrolling.")})).collect();
                activity.items.push(json!({"id":"child-command","type":"commandExecution","command":"cargo test retry","status":"inProgress","aggregatedOutput":"running tests"}));
                session.state.subagent_activity.insert("child".into(), activity);
                cx.notify();
            });
        }));
        draw(cx);
        draw(cx);
        assert!(cx.debug_bounds("codex-composer").is_none());
        assert!(cx.debug_bounds("codex-edit-message-0").is_none());
        let toggle = cx
            .debug_bounds("codex-toggle-tool-24")
            .expect("child uses the shared tool disclosure");
        cx.simulate_click(toggle.center(), gpui::Modifiers::default());
        draw(cx);
        view.read_with(cx, |view, cx| {
            let state = view.session.read(cx).state.subagent_snapshot("child");
            assert!(
                tool_is_running(&state, 24),
                "shared shimmer predicate sees the active child tool"
            );
            assert!(view
                .expanded
                .contains(&tool_expansion_key(&state.saved.items[24], 24)));
            assert!(view.scroll.offset().y < gpui::Pixels::ZERO);
        });
        cx.update(|_, cx| view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                let activity = session.state.subagent_activity.get_mut("child").unwrap();
                activity.items[24]["status"] = json!("completed");
                activity.items.push(json!({"id":"child-latest","type":"agentMessage","text":"Latest child response after the completed command."}));
                cx.notify();
            });
        }));
        draw(cx);
        draw(cx);
        let latest = cx
            .debug_bounds("codex-message-anchor-25")
            .expect("incoming child item rendered");
        let transcript = cx.debug_bounds("codex-messages").unwrap();
        assert!(latest.bottom() <= transcript.bottom() + gpui::px(1.));
        assert!(latest.bottom() > transcript.top());
        view.read_with(cx, |view, cx| {
            let state = view.session.read(cx).state.subagent_snapshot("child");
            assert!(!tool_is_running(&state, 24));
        });
    }

    #[gpui::test]
    fn restored_child_preview_exposes_saved_work_without_live_shimmer(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            cx.update(|_, cx| {
                cx.set_reduce_motion(true);
                view.update(cx, |view, cx| {
                    view.preview_child = Some("saved-child".into());
                    view.expanded.clear();
                    view.session.update(cx, |session, cx| {
                        session.state.saved.provider = provider;
                        let mut activity = codex::SubagentActivity::default();
                        activity.items = vec![
                            json!({"id":"saved-commentary", "type":"agentMessage", "phase":"commentary", "text":"Inspecting the saved retry path."}),
                            json!({"id":"saved-tool", "type":"commandExecution", "command":"cargo test retry", "status":"inProgress", "aggregatedOutput":"running tests before disconnect"}),
                            json!({"id":"saved-answer", "type":"agentMessage", "phase":"final_answer", "text":"The saved result remains available."}),
                        ];
                        session.state.subagent_activity.insert("saved-child".into(), activity);
                        cx.notify();
                    });
                });
            });
            draw(cx);
            draw(cx);
            let work = cx
                .debug_bounds("codex-work-header-0")
                .expect("restored child work remains reachable through the shared disclosure");
            cx.simulate_click(work.center(), gpui::Modifiers::default());
            draw(cx);
            assert!(
                cx.debug_bounds("codex-message-anchor-0").is_some(),
                "{provider:?}: saved commentary expands"
            );
            let tool = cx
                .debug_bounds("codex-toggle-tool-1")
                .expect("saved child tool appears after expanding its work");
            cx.simulate_click(tool.center(), gpui::Modifiers::default());
            draw(cx);
            view.read_with(cx, |view, cx| {
                let state = view.session.read(cx).state.subagent_snapshot("saved-child");
                assert_eq!(state.status, Status::Idle);
                assert!(
                    !tool_is_running(&state, 1),
                    "{provider:?}: saved in-progress status does not imply live work"
                );
                assert!(view
                    .expanded
                    .contains(&tool_expansion_key(&state.saved.items[1], 1)));
            });
        }
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
                    updates: receive.into(),
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
        state.permission_modes = [
            "default",
            "acceptEdits",
            "plan",
            "dontAsk",
            "bypassPermissions",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let (choices, _) = CodexView::menu_choices(&state, ComposerMenu::Access);
        assert!(choices.iter().any(|(value, _)| value == "default"));
        assert!(!choices
            .iter()
            .any(|(value, _)| value == "bypassPermissions"));
        assert!(!choices
            .iter()
            .any(|(value, label)| value == "workspace-write" || label.contains("sandbox")));
        for flag in [
            "--dangerously-skip-permissions",
            "--allow-dangerously-skip-permissions",
        ] {
            state.saved.config_args = vec![flag.into()];
            let (choices, _) = CodexView::menu_choices(&state, ComposerMenu::Access);
            assert!(choices
                .iter()
                .any(|(value, _)| value == "bypassPermissions"));
        }
    }
    #[test]
    fn yolo_identity_comes_from_saved_native_launch_options() {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let mut state = snapshot();
            state.saved.provider = provider;
            state.access = Some(
                if provider == codex::Provider::Codex {
                    "full-access"
                } else {
                    "bypassPermissions"
                }
                .into(),
            );
            state.resolved_access = state.access.clone();
            state.sandbox = Some(json!({"type":"dangerFullAccess"}));
            state.approval_policy = Some(json!("never"));
            assert!(!session_launched_with_yolo(&state));
            state.saved.config_args = provider
                .launch_options(&provider.agent().launch_args_for_backend(true, true, false))
                .unwrap()
                .0;
            let saved =
                serde_json::from_value(serde_json::to_value(&state.saved).unwrap()).unwrap();
            state = Snapshot::new(saved);
            assert!(session_launched_with_yolo(&state));
            state.access = Some("plan".into());
            state.resolved_access = Some("plan".into());
            assert!(
                session_launched_with_yolo(&state),
                "effective provider fallback does not rewrite launch identity"
            );
            assert!(!session_has_yolo_access(&state));
            state.saved.native_config_version = 0;
            assert!(
                !session_launched_with_yolo(&state),
                "legacy CLI bypass is not a native Yolo launch"
            );
        }
    }

    #[test]
    fn yolo_readonly_requires_launch_provenance_and_effective_bypass() {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let mut state = snapshot();
            state.saved.provider = provider;
            state.saved.config_args = provider
                .launch_options(&provider.agent().launch_args_for_backend(true, true, false))
                .unwrap()
                .0;
            if provider == codex::Provider::Codex {
                state.resolved_access = Some("full-access".into());
                state.sandbox = Some(json!({"type":"dangerFullAccess"}));
                state.approval_policy = Some(json!("on-request"));
                assert!(!session_has_yolo_access(&state));
                state.approval_policy = Some(json!("never"));
                assert!(session_has_yolo_access(&state));
                state.sandbox = Some(json!({"type":"readOnly"}));
                assert!(!session_has_yolo_access(&state));
            } else {
                state.resolved_access = Some("default".into());
                assert!(!session_has_yolo_access(&state));
                state.resolved_access = Some("bypassPermissions".into());
                assert!(session_has_yolo_access(&state));
                state.access = Some("default".into());
                assert!(!session_has_yolo_access(&state));
            }
        }
    }

    #[gpui::test]
    fn access_control_wraps_with_its_chevron_inside_composer(cx: &mut gpui::TestAppContext) {
        let (view, cx, _, _) = fixture(cx);
        for (width, rem) in [(600.0, 16.0), (400.0, 16.0), (400.0, 20.0)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.0)));
            cx.update(|_, cx| {
                gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, cx| {
                        session.state.sandbox = Some(json!({"type":"dangerFullAccess"}));
                        session.state.approval_policy = Some(json!("untrusted"));
                        cx.notify();
                    });
                });
            });
            draw(cx);
            let composer = cx.debug_bounds("codex-composer").unwrap();
            let label = cx.debug_bounds("codex-access-summary").unwrap();
            let indicator = cx.debug_bounds("codex-access-indicator").unwrap();
            let control = view.read_with(cx, |view, _| view.option_bounds[2].get());
            let inset =
                cx.update(|window, _| rpx(SPACE_2XL).to_pixels(window.rem_size()) + gpui::px(1.));
            assert!(
                control.left() >= composer.left() + inset
                    && control.right() <= composer.right() - inset
            );
            assert!(label.right() <= indicator.left());
            assert!(indicator.right() <= control.right());
            assert!(indicator.top() >= control.top() && indicator.bottom() <= control.bottom());
        }
    }

    #[gpui::test]
    fn explicit_full_access_remains_reversible_for_codex_and_claude(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(crate::settings::SettingsState::new(
                grove_core::storage::Store::default(),
            ));
        });
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            for restored in [false, true] {
                let (view, cx, actions, _) = fixture(cx);
                cx.update(|_, cx| {
                    view.update(cx, |view, cx| {
                        view.session.update(cx, |session, cx| {
                            let access = if provider == codex::Provider::Codex {
                                "full-access"
                            } else {
                                "bypassPermissions"
                            };
                            session.state.saved.provider = provider;
                            session.state.saved.composer.access = Some(access.into());
                            if restored {
                                let saved = serde_json::from_value(
                                    serde_json::to_value(&session.state.saved).unwrap(),
                                )
                                .unwrap();
                                session.state = Snapshot::new(saved).into();
                            }
                            // Restored sessions publish their connected state after startup.
                            session.state.status = Status::Idle;
                            session.state.access = Some(access.into());
                            session.state.resolved_access = Some(access.into());
                            session.state.permission_modes =
                                ["default", "acceptEdits", "bypassPermissions"]
                                    .map(str::to_owned)
                                    .to_vec();
                            cx.notify();
                        });
                    });
                });
                for enabled in [false, true] {
                    cx.update(|_, cx| {
                        cx.global_mut::<crate::settings::SettingsState>()
                            .store
                            .yolo_enabled = enabled;
                    });
                    draw(cx);
                    assert!(
                        cx.debug_bounds("codex-yolo-access").is_none(),
                        "new-session Settings preferences must not relabel an existing session"
                    );
                }
                draw(cx);
                assert!(
                    cx.debug_bounds("codex-yolo-access").is_none(),
                    "an explicit access choice must not turn into a Yolo launch"
                );
                let trigger = view.read_with(cx, |view, _| view.option_bounds[2].get());
                cx.simulate_click(trigger.center(), gpui::Modifiers::default());
                draw(cx);
                let popup = cx.debug_bounds("codex-options-popup").unwrap();
                // Both providers have three choices plus configured defaults.
                let first_row =
                    gpui::point(popup.center().x, popup.top() + popup.size.height * 0.125);
                cx.simulate_click(first_row, gpui::Modifiers::default());
                draw(cx);
                let expected = if provider == codex::Provider::Codex {
                    "read-only"
                } else {
                    "default"
                };
                assert!(actions.try_iter().any(|action| matches!(action, Action::Configure { access: Some(access), .. } if access == expected)));
                assert!(view.read_with(cx, |view, _| view.popup.is_none()));
            }
        }
    }

    #[gpui::test]
    fn yolo_access_is_readonly_and_cannot_open_access_menu(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(crate::settings::SettingsState::new(
                grove_core::storage::Store::default(),
            ));
        });
        let (view, cx, actions, _) = fixture(cx);
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, cx| {
                        session.state.saved.provider = provider;
                        session.state.saved.config_args = provider
                            .launch_options(
                                &provider.agent().launch_args_for_backend(true, true, false),
                            )
                            .unwrap()
                            .0;
                        session.state.access = Some(
                            if provider == codex::Provider::Codex {
                                "full-access"
                            } else {
                                "bypassPermissions"
                            }
                            .into(),
                        );
                        cx.notify();
                    });
                    view.open_menu(ComposerMenu::Access, window, cx);
                    assert!(view.popup.is_none());
                    assert!(view.menu.is_none());
                });
            });
            for enabled in [false, true] {
                cx.update(|_, cx| {
                    cx.global_mut::<crate::settings::SettingsState>()
                        .store
                        .yolo_enabled = enabled;
                });
                draw(cx);
                assert!(cx.debug_bounds("codex-yolo-access").is_some());
                assert!(cx.debug_bounds("codex-access-summary").is_none());
            }
        }
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::Configure { .. })));
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
            rich_plans: grove_core::rich_plan::PlanStore::default(),
            claude_resume: None,
            claude_subagents: std::collections::HashMap::default(),
            edit_checkpoints: Vec::new(),
            pending_edit: None,
            provider: grove_core::codex::Provider::Codex,
            composer: grove_core::codex::ComposerSettings::default(),
            context: None,
            work_periods: Vec::new(),
            key: "native-ui-test".into(),
            project: "Demo".into(),
            cwd: "/unused".into(),
            label: "Codex".into(),
            title: None,
            title_generation: None,
            roots: Vec::new(),
            thread_id: Some("thread-ui".into()),
            items: Vec::new(),
            draft: String::new(),
            draft_images: Vec::new(),
            draft_reference: None,
            config_args: Vec::new(),
            native_config_version: 1,
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
    fn fixture(
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
    #[gpui::test]
    fn speed_keyboard_selection_preserves_composer_and_rejects_unavailable(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| view.update(cx, |view, cx| {
            view.session.update(cx, |session, _| {
                session.state.models = vec![json!({"model":"speed-model", "serviceTiers":[{"id":"fast"}], "supportedReasoningEfforts":[]})];
                session.state.model = Some("speed-model".into());
                session.state.saved.draft_images = vec!["untouched attachment".into()];
            });
            view.input.update(cx, |input, cx| input.set_value("Keep my speed draft", window, cx));
            view.open_menu(ComposerMenu::Settings, window, cx);
            view.speed_focus[1].focus(window, cx);
        }));
        draw(cx);
        let _ = actions.try_iter().collect::<Vec<_>>();
        cx.simulate_keystrokes("space");
        draw(cx);
        assert!(actions
            .try_iter()
            .any(|action| matches!(action, Action::SetSpeed(codex::Speed::Fast))));
        cx.update(|_, cx| view.update(cx, |view, cx| view.send_speed(codex::Speed::Ultrafast, cx)));
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::SetSpeed(_))));
        cx.simulate_keystrokes("escape");
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert!(view.menu.is_none());
            assert_eq!(view.input.read(cx).value().as_ref(), "Keep my speed draft");
            assert_eq!(
                view.session.read(cx).state.saved.draft_images,
                ["untouched attachment"]
            );
        });
    }

    #[gpui::test]
    fn speed_card_stays_above_composer_at_narrow_width_and_zoom(cx: &mut gpui::TestAppContext) {
        let (view, cx, _, _) = fixture(cx);
        for (width, rem) in [(1024.0, 16.0), (400.0, 16.0), (400.0, 24.0)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(720.0)));
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    window.set_rem_size(gpui::px(rem));
                    view.close_menu(cx);
                    view.session.update(cx, |session, _| {
                        session.state.saved.composer.speed = Some(codex::Speed::Fast);
                        session.state.models.clear();
                    });
                });
            });
            draw(cx);
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.open_menu(ComposerMenu::Settings, window, cx);
                });
            });
            draw(cx);
            let card = cx.debug_bounds("codex-options-popup").unwrap();
            let trigger = view.read_with(cx, |view, _| view.option_bounds[0].get());
            assert!(card.top() >= gpui::px(0.0), "card top {card:?}, rem={rem}");
            assert!(
                card.bottom() <= trigger.top(),
                "card overlaps composer {card:?}, trigger={trigger:?}, rem={rem}"
            );
            assert!(
                card.left() >= gpui::px(0.0) && card.right() <= gpui::px(width),
                "card exceeds width {card:?}"
            );
            assert!(cx.debug_bounds("codex-speed-0").is_some());
        }
    }

    #[test]
    #[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
    #[cfg(target_os = "macos")]
    fn speed_card_native_snapshots() {
        use gpui::AppContext as _;
        let mut cx = gpui::HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            Arc::new(crate::assets::Assets),
            gpui_platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_reduce_motion(true);
        });
        let previous_theme = grove_core::theme::current().name.clone();
        for appearance in ["dark", "light"] {
            cx.update(|cx| {
                assert!(grove_core::theme::set_by_name(&format!(
                    "grove-{appearance}"
                )));
                c::sync_component_theme(cx);
            });
            for (width, rem) in [(1024.0, 16.0), (400.0, 16.0), (400.0, 24.0)] {
                for provider in [codex::Provider::Codex, codex::Provider::Claude] {
                    for available in [true, false] {
                        let mut state = snapshot();
                        state.saved.provider = provider;
                        state.saved.draft = "Compare the two refresh paths.".into();
                        state.saved.items.clear();
                        state.saved.composer.speed = Some(codex::Speed::Fast);
                        state.model = Some("speed-fixture".into());
                        state.effort = Some("high".into());
                        state.models = vec![
                            json!({"model":"speed-fixture", "displayName":if available {"Selected model"} else {"A very long configured model name with unavailable fast mode"}, "supportsFastMode":available, "serviceTiers":if available {vec![json!({"id":"fast"})]} else {vec![]}, "supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"medium"},{"reasoningEffort":"high"}]}),
                        ];
                        if provider == codex::Provider::Claude && available {
                            state.reported_speed = Some("cooldown".into());
                        }
                        let (handle, _actions) = Handle::channel();
                        let (_updates, receive) = mpsc::channel();
                        let connection = Connection {
                            handle,
                            updates: receive.into(),
                        };
                        let mut retained = None;
                        let window = cx
                            .open_window(
                                gpui::size(gpui::px(width), gpui::px(720.0)),
                                |window, cx| {
                                    window.set_rem_size(gpui::px(rem));
                                    let session = cx.new(|cx| {
                                        CodexSession::from_connection(state, connection, cx)
                                    });
                                    let chat = cx.new(|cx| CodexView::new(session, window, cx));
                                    retained = Some(chat.clone());
                                    cx.new(|cx| {
                                        gpui_component::Root::new(chat, window, cx)
                                            .bordered(false)
                                            .bg(c::BG())
                                    })
                                },
                            )
                            .unwrap();
                        cx.update_window(window.into(), |_, window, cx| {
                            retained.as_ref().unwrap().update(cx, |view, cx| {
                                view.open_menu(ComposerMenu::Settings, window, cx);
                            });
                        })
                        .unwrap();
                        for _ in 0..4 {
                            cx.advance_clock(std::time::Duration::from_millis(16));
                            cx.run_until_parked();
                            cx.update_window(window.into(), |_, window, cx| {
                                window.simulate_next_frame(cx);
                                let _ = window.draw(cx);
                            })
                            .unwrap();
                        }
                        cx.update_window(window.into(), |_, _window, cx| {
                            let view = retained.as_ref().unwrap().read(cx);
                            assert_eq!(
                                view.input.read(cx).value().as_ref(),
                                "Compare the two refresh paths."
                            );
                        })
                        .unwrap();
                        let path = format!(
                            "/tmp/grove-speed-{appearance}-{provider:?}-{width:.0}-{rem:.0}-{available}.png"
                        );
                        cx.capture_screenshot(window.into())
                            .unwrap()
                            .save(&path)
                            .unwrap();
                        eprintln!("Native speed card fixture: {path}");
                        cx.update_window(window.into(), |_, window, _| window.remove_window())
                            .unwrap();
                    }
                }
            }
        }
        cx.update(|cx| {
            assert!(grove_core::theme::set_by_name(&previous_theme));
            c::sync_component_theme(cx);
        });
    }

    #[test]
    #[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
    #[cfg(target_os = "macos")]
    fn effort_card_native_snapshots() {
        use gpui::AppContext as _;
        let mut cx = gpui::HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            Arc::new(crate::assets::Assets),
            gpui_platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_reduce_motion(true);
        });
        let previous_theme = grove_core::theme::current().name.clone();
        for appearance in ["dark", "light"] {
            cx.update(|cx| {
                assert!(grove_core::theme::set_by_name(&format!(
                    "grove-{appearance}"
                )));
                c::sync_component_theme(cx);
            });
            for (width, rem) in [(1024.0, 16.0), (400.0, 16.0), (400.0, 24.0)] {
                cx.update(|cx| gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem));
                for provider in [codex::Provider::Codex, codex::Provider::Claude] {
                    for effort in ["low", "high"] {
                        let mut state = snapshot();
                        state.saved.provider = provider;
                        state.saved.draft = "Compare the two refresh paths.".into();
                        state.saved.items.clear();
                        state.saved.composer.speed = Some(codex::Speed::Fast);
                        state.model = Some("speed-fixture".into());
                        state.effort = Some(effort.into());
                        state.models = vec![
                            json!({"model":"speed-fixture", "displayName":"Selected model", "supportsFastMode":true, "serviceTiers":vec![json!({"id":"fast"})], "supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"medium"},{"reasoningEffort":"high"}]}),
                        ];
                        if provider == codex::Provider::Claude {
                            state.reported_speed = Some("cooldown".into());
                        }
                        let (handle, _actions) = Handle::channel();
                        let (_updates, receive) = mpsc::channel();
                        let connection = Connection {
                            handle,
                            updates: receive.into(),
                        };
                        let mut retained = None;
                        let window = cx
                            .open_window(
                                gpui::size(gpui::px(width), gpui::px(720.0)),
                                |window, cx| {
                                    window.set_rem_size(gpui::px(rem));
                                    let session = cx.new(|cx| {
                                        CodexSession::from_connection(state, connection, cx)
                                    });
                                    let chat = cx.new(|cx| CodexView::new(session, window, cx));
                                    retained = Some(chat.clone());
                                    cx.new(|cx| {
                                        gpui_component::Root::new(chat, window, cx)
                                            .bordered(false)
                                            .bg(c::BG())
                                    })
                                },
                            )
                            .unwrap();
                        cx.update_window(window.into(), |_, window, cx| {
                            retained.as_ref().unwrap().update(cx, |view, cx| {
                                view.open_menu(ComposerMenu::Settings, window, cx);
                            });
                        })
                        .unwrap();
                        for _ in 0..4 {
                            cx.advance_clock(std::time::Duration::from_millis(16));
                            cx.run_until_parked();
                            cx.update_window(window.into(), |_, window, cx| {
                                window.simulate_next_frame(cx);
                                let _ = window.draw(cx);
                            })
                            .unwrap();
                        }
                        cx.update_window(window.into(), |_, window, cx| {
                            assert_eq!(
                                window.rem_size(),
                                gpui::px(rem),
                                "native fixture must render the requested zoom"
                            );
                            let view = retained.as_ref().unwrap().read(cx);
                            assert_eq!(
                                view.input.read(cx).value().as_ref(),
                                "Compare the two refresh paths."
                            );
                        })
                        .unwrap();
                        let path = format!(
                            "/tmp/grove-effort-{appearance}-{provider:?}-{width:.0}-{rem:.0}-{effort}.png"
                        );
                        let screenshot = cx.capture_screenshot(window.into()).unwrap();
                        screenshot.save(&path).unwrap();
                        if effort == "low" {
                            // The larger thumb fully covers a correctly rounded track cap.
                            // A short fill with radius clamped by its width leaks a crescent.
                            let (bounds, viewport, blue) = cx
                                .update_window(window.into(), |_, window, cx| {
                                    let bounds =
                                        retained.as_ref().unwrap().read(cx).effort_bounds.get();
                                    (bounds, window.viewport_size(), gpui::Rgba::from(c::BLUE()))
                                })
                                .unwrap();
                            let scale = screenshot.width() as f32 / f32::from(viewport.width);
                            let inset = ICON_MD * rem / crate::zoom::REM_BASE;
                            let blue_pixels = screenshot
                                .enumerate_pixels()
                                .filter(|(x, y, pixel)| {
                                    let x = *x as f32 / scale;
                                    let y = *y as f32 / scale;
                                    x >= f32::from(bounds.left()) - inset
                                        && x <= f32::from(bounds.right()) + inset
                                        && y >= f32::from(bounds.top()) - inset
                                        && y <= f32::from(bounds.bottom()) + inset
                                        && (f32::from(pixel[0]) / 255.0 - blue.r).abs() < 0.02
                                        && (f32::from(pixel[1]) / 255.0 - blue.g).abs() < 0.02
                                        && (f32::from(pixel[2]) / 255.0 - blue.b).abs() < 0.02
                                })
                                .count();
                            assert_eq!(
                                blue_pixels, 0,
                                "low effort must not leak a blue cap behind the thumb: {path}"
                            );
                        }
                        eprintln!("Native effort card fixture: {path}");
                        cx.update_window(window.into(), |_, window, _| window.remove_window())
                            .unwrap();
                    }
                }
            }
        }
        cx.update(|cx| {
            assert!(grove_core::theme::set_by_name(&previous_theme));
            c::sync_component_theme(cx);
        });
    }

    #[test]
    #[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
    #[cfg(target_os = "macos")]
    fn composer_markdown_native_snapshots() {
        use gpui::AppContext as _;
        let mut cx = gpui::HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            Arc::new(crate::assets::Assets),
            gpui_platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_component::init(cx);
            c::sync_component_theme(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_reduce_motion(true);
        });
        for width in [1024.0, 600.0, 360.0] {
            for provider in [codex::Provider::Codex, codex::Provider::Claude] {
                let mut state = snapshot();
                state.saved.provider = provider;
                state.saved.items = vec![
                    json!({"type":"agentMessage", "id":"snippet", "text":"Copy this code:\n\n```sh\n  cargo test --workspace\n```", "groveCompleted":true}),
                ];
                state.saved.draft = "## Heading 2\n- This is a **bold** list item\n- Second item with `code` and [link](https://example.com)\n1. Ordered 🦀 é item\nNormal paragraph after heading and lists, with enough additional Unicode text é🦀 to wrap at narrow widths.".into();
                let (handle, _actions) = Handle::channel();
                let (_updates, receive) = mpsc::channel();
                let connection = Connection {
                    handle,
                    updates: receive.into(),
                };
                let mut retained = None;
                let window = cx
                    .open_window(
                        gpui::size(gpui::px(width), gpui::px(600.0)),
                        |window, cx| {
                            let session =
                                cx.new(|cx| CodexSession::from_connection(state, connection, cx));
                            let chat = cx.new(|cx| CodexView::new(session, window, cx));
                            retained = Some(chat.clone());
                            cx.new(|cx| {
                                gpui_component::Root::new(chat, window, cx)
                                    .bordered(false)
                                    .bg(c::BG())
                            })
                        },
                    )
                    .unwrap();
                {
                    cx.update_window(window.into(), |_, window, cx| {
                        retained.as_ref().unwrap().update(cx, |view, cx| {
                            view.input.update(cx, |input, cx| {
                                let source = "## Heading 2\n- This is a **bold** list item\n> 9. [x] Completed é🦀";
                                input.set_value(source, window, cx);
                                input.set_selected_range(source.len()..source.len(), cx);
                                input.focus(window, cx);
                            });
                        });
                        window.dispatch_action(Box::new(gpui_component::input::Enter { secondary: false, shift: true }), cx);
                    }).unwrap();
                    cx.run_until_parked();
                    cx.update(|cx| {
                        let view = retained.as_ref().unwrap().read(cx);
                        assert!(view.input.read(cx).value().ends_with("\n> 10. [ ] "));
                    });
                    for _ in 0..4 {
                        cx.advance_clock(std::time::Duration::from_millis(16));
                        cx.run_until_parked();
                        cx.update_window(window.into(), |_, window, cx| {
                            window.simulate_next_frame(cx);
                            let _ = window.draw(cx);
                        })
                        .unwrap();
                    }
                    let path = format!("/tmp/grove-composer-markdown-{provider:?}-{width:.0}.png");
                    cx.capture_screenshot(window.into())
                        .unwrap()
                        .save(&path)
                        .unwrap();
                    eprintln!("Native composer Markdown fixture: {path}");
                }
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
    }

    #[test]
    #[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
    #[cfg(target_os = "macos")]
    fn snippet_copy_native_snapshots() {
        use gpui::AppContext as _;
        let mut cx = gpui::HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            Arc::new(crate::assets::Assets),
            gpui_platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_component::init(cx);
            c::sync_component_theme(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_reduce_motion(true);
        });
        for width in [1024.0, 600.0] {
            for provider in [codex::Provider::Codex, codex::Provider::Claude] {
                let mut state = snapshot();
                state.saved.provider = provider;
                state.saved.items = vec![
                    json!({"type":"agentMessage", "id":"snippet", "text":"Copy this code:\n\n```sh\n  cargo test --workspace\n```", "groveCompleted":true}),
                ];
                let thread = state.saved.thread_id.clone().unwrap_or_default();
                let (handle, _actions) = Handle::channel();
                let (_updates, receive) = mpsc::channel();
                let connection = Connection {
                    handle,
                    updates: receive.into(),
                };
                let mut retained = None;
                let window = cx
                    .open_window(
                        gpui::size(gpui::px(width), gpui::px(600.0)),
                        |window, cx| {
                            let session =
                                cx.new(|cx| CodexSession::from_connection(state, connection, cx));
                            let chat = cx.new(|cx| CodexView::new(session, window, cx));
                            retained = Some(chat.clone());
                            cx.new(|cx| {
                                gpui_component::Root::new(chat, window, cx)
                                    .bordered(false)
                                    .bg(c::BG())
                            })
                        },
                    )
                    .unwrap();
                for copied in [false, true] {
                    if copied {
                        cx.update_window(window.into(), |_, _, cx| {
                            retained.as_ref().unwrap().update(cx, |view, cx| {
                                view.copy_code_block(
                                    format!("{thread}:snippet:code:17"),
                                    "  cargo test --workspace\n".into(),
                                    cx,
                                );
                            });
                        })
                        .unwrap();
                    }
                    for _ in 0..4 {
                        cx.advance_clock(std::time::Duration::from_millis(16));
                        cx.run_until_parked();
                        cx.update_window(window.into(), |_, window, cx| {
                            window.simulate_next_frame(cx);
                            let _ = window.draw(cx);
                        })
                        .unwrap();
                    }
                    let path =
                        format!("/tmp/grove-snippet-copy-{provider:?}-{width:.0}-{copied}.png");
                    cx.capture_screenshot(window.into())
                        .unwrap()
                        .save(&path)
                        .unwrap();
                    eprintln!("Native snippet copy fixture: {path}");
                }
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
    }

    #[test]
    #[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
    #[cfg(target_os = "macos")]
    fn composer_access_native_snapshots() {
        use gpui::AppContext as _;
        let mut cx = gpui::HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            Arc::new(crate::assets::Assets),
            gpui_platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_component::init(cx);
            c::sync_component_theme(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_reduce_motion(true);
        });
        let mut baseline_heights = HashMap::new();
        for (width, rem) in [
            (1024.0, 16.0),
            (600.0, 16.0),
            (600.0, 20.0),
            (400.0, 16.0),
            (400.0, 20.0),
        ] {
            // Root reapplies the component theme's font size on every render.
            // Set its source of truth rather than a rem value Root will overwrite.
            cx.update(|cx| gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem));
            for kind in ["configured", "codex-yolo", "claude-yolo"] {
                let mut state = snapshot();
                state.sandbox = Some(json!({"type":"dangerFullAccess"}));
                state.approval_policy = Some(json!("untrusted"));
                if kind == "codex-yolo" {
                    state.access = Some("full-access".into());
                } else if kind == "claude-yolo" {
                    state.saved.provider = codex::Provider::Claude;
                    state.access = Some("bypassPermissions".into());
                }
                if kind != "configured" {
                    let provider = state.saved.provider;
                    state.saved.config_args = provider
                        .launch_options(
                            &provider.agent().launch_args_for_backend(true, true, false),
                        )
                        .unwrap()
                        .0;
                }
                let (handle, _actions) = Handle::channel();
                let (_updates, receive) = mpsc::channel();
                let connection = Connection {
                    handle,
                    updates: receive.into(),
                };
                let mut retained = None;
                let window = cx
                    .open_window(
                        gpui::size(gpui::px(width), gpui::px(800.0)),
                        |window, cx| {
                            let session =
                                cx.new(|cx| CodexSession::from_connection(state, connection, cx));
                            let chat = cx.new(|cx| CodexView::new(session, window, cx));
                            retained = Some(chat.clone());
                            cx.new(|cx| {
                                gpui_component::Root::new(chat, window, cx)
                                    .bordered(false)
                                    .bg(c::BG())
                            })
                        },
                    )
                    .unwrap();
                for _ in 0..12 {
                    cx.advance_clock(std::time::Duration::from_millis(16));
                    cx.run_until_parked();
                    cx.update_window(window.into(), |_, window, cx| {
                        window.simulate_next_frame(cx);
                        let _ = window.draw(cx);
                    })
                    .unwrap();
                }
                cx.update_window(window.into(), |_, window, cx| {
                    assert_eq!(
                        window.rem_size(),
                        gpui::px(rem),
                        "Root must retain requested zoom"
                    );
                    let view = retained.as_ref().unwrap().read(cx);
                    let model = view.option_bounds[0].get();
                    assert!(model.left() >= gpui::px(0.) && model.right() <= gpui::px(width));
                    if rem == 16.0 {
                        baseline_heights.insert((kind, width as u32), model.size.height);
                    } else {
                        let baseline = baseline_heights[&(kind, width as u32)];
                        assert!(
                            model.size.height > baseline,
                            "zoom must change actual control geometry: {model:?} vs {baseline:?}"
                        );
                    }
                    if kind == "configured" {
                        let access = view.option_bounds[2].get();
                        assert!(
                            access.left() >= rpx(SPACE_20 + SPACE_2XL).to_pixels(window.rem_size()) + gpui::px(1.)
                                && access.right() <= gpui::px(width) - rpx(SPACE_20 + SPACE_2XL).to_pixels(window.rem_size()) - gpui::px(1.),
                            "entire access button, including its reserved chevron, must fit composer content: {access:?}"
                        );
                        assert!(
                            access.left() >= model.right() || access.top() >= model.bottom(),
                            "wrapping controls must not overlap: {model:?}, {access:?}"
                        );
                    }
                    eprintln!(
                        "Native composer geometry {kind}/{width}/{rem}: model={model:?}, rem={:?}",
                        window.rem_size()
                    );
                })
                .unwrap();
                let path = format!("/tmp/grove-composer-access-{kind}-{width:.0}-{rem:.0}.png");
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(&path)
                    .unwrap();
                eprintln!("Native composer access fixture: {path}");
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
    }

    /// Opt-in real text shaping / Metal snapshots, independent of installed Grove.
    #[test]
    #[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
    #[cfg(target_os = "macos")]
    fn approval_tray_native_snapshots() {
        use gpui::AppContext as _;
        let mut cx = gpui::HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            Arc::new(crate::assets::Assets),
            gpui_platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_component::init(cx);
            c::sync_component_theme(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_reduce_motion(true);
        });
        for width in [1024.0, 600.0] {
            for kind in ["computer", "command", "command-prefix", "claude"] {
                let mut state = snapshot();
                state.status = Status::Waiting;
                state.saved.items = vec![
                    json!({"id":"intro","type":"agentMessage","text":"I’ll inspect the launcher’s current layout."}),
                    json!({"id":"plan","type":"plan","plan":[{"step":"Inspect launcher","status":"completed"},{"step":"Verify interaction states","status":"inProgress"}]}),
                ];
                if kind == "computer" {
                    state.elicitations.push(ElicitationRequest::parse_codex(json!("computer"), &json!({
                        "serverName":"cua_repl", "message":"Allow Computer Use to use \"Grove Launcher QA\"?",
                        "requestedSchema":{"type":"object","properties":{}}
                    })).unwrap());
                } else if kind == "claude" {
                    state.saved.provider = codex::Provider::Claude;
                    state.approvals.push(json!({"id":"claude", "method":"item/commandExecution/requestApproval",
                        "groveAgent":"Reviewer", "claudeInput":{"command":"cargo test --lib controls::tests", "description":"Verify approval response mapping"},
                        "claudeRequest":{"tool_name":"Bash","input":{"command":"cargo test --lib controls::tests"}}
                    }));
                } else {
                    state.approvals.push(json!({"id":"command","method":"item/commandExecution/requestApproval","params":{
                        "reason":"May I resolve the vendored selection test dependencies to compile and run its focused regression tests?",
                        "command":"/bin/zsh -lc 'cargo test --manifest-path vendor/gpui-component/ui/Cargo.toml --lib text::window_selection::tests --no-run --locked'",
                        "cwd":"/Users/digvijaymahapatra/Sandbox/grove",
                        "proposedExecpolicyAmendment":["cargo","test","--manifest-path","vendor/gpui-component/ui/Cargo.toml","--lib","text::window_selection::tests","--no-run","--locked"]
                    }}));
                }
                let (handle, _actions) = Handle::channel();
                let (_updates, receive) = mpsc::channel();
                let connection = Connection {
                    handle,
                    updates: receive.into(),
                };
                let mut capture_view = None;
                let window = cx
                    .open_window(
                        gpui::size(gpui::px(width), gpui::px(800.0)),
                        |window, cx| {
                            let session =
                                cx.new(|cx| CodexSession::from_connection(state, connection, cx));
                            let chat = cx.new(|cx| CodexView::new(session, window, cx));
                            capture_view = Some(chat.clone());
                            cx.new(|cx| {
                                gpui_component::Root::new(chat, window, cx)
                                    .bordered(false)
                                    .bg(c::BG())
                            })
                        },
                    )
                    .unwrap();
                cx.run_until_parked();
                cx.update_window(window.into(), |_, window, cx| {
                    if kind == "command-prefix" {
                        capture_view.as_ref().unwrap().update(cx, |view, cx| {
                            for draft in view.approval_drafts.values_mut() {
                                draft.policy_expanded = true;
                            }
                            cx.notify();
                        });
                    }
                    let _ = window.draw(cx);
                })
                .unwrap();
                let path = format!("/tmp/grove-permission-{kind}-{width:.0}.png");
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(&path)
                    .unwrap();
                eprintln!("Native approval fixture: {path}");
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
    }

    /// Opt-in real text shaping / Metal snapshots, independent of installed Grove.
    #[test]
    #[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
    #[cfg(target_os = "macos")]
    fn transcript_activity_native_snapshots() {
        use gpui::AppContext as _;
        let mut cx = gpui::HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            Arc::new(crate::assets::Assets),
            gpui_platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_component::init(cx);
            c::sync_component_theme(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_reduce_motion(true);
        });
        for width in [1024.0, 600.0] {
            for kind in ["codex", "claude"] {
                let mut state = snapshot();
                state.status = Status::Working;
                if kind == "claude" {
                    state.saved.provider = codex::Provider::Claude;
                }
                state.saved.items = vec![
                    json!({"id":"question","type":"userMessage","content":[{"type":"text","text":"Inspect the working tree"}]}),
                    json!({"id":"think","type":"reasoning","text":"Checking files","status":"completed"}),
                    json!({"id":"done","type":"commandExecution","command":"pwd","status":"completed","aggregatedOutput":"/workspace"}),
                    json!({"id":"failed","type":"commandExecution","command":"git fetch","status":"failed","exitCode":1}),
                    json!({"id":"active","type":"commandExecution","command":"git status --short","status":"inProgress"}),
                ];
                let (handle, _actions) = Handle::channel();
                let (_updates, receive) = mpsc::channel();
                let connection = Connection {
                    handle,
                    updates: receive.into(),
                };
                let mut capture_view = None;
                let window = cx
                    .open_window(
                        gpui::size(gpui::px(width), gpui::px(800.0)),
                        |window, cx| {
                            let session =
                                cx.new(|cx| CodexSession::from_connection(state, connection, cx));
                            let chat = cx.new(|cx| CodexView::new(session, window, cx));
                            capture_view = Some(chat.clone());
                            cx.new(|cx| {
                                gpui_component::Root::new(chat, window, cx)
                                    .bordered(false)
                                    .bg(c::BG())
                            })
                        },
                    )
                    .unwrap();
                cx.run_until_parked();
                cx.update_window(window.into(), |_, window, cx| {
                    capture_view.as_ref().unwrap().update(cx, |view, cx| {
                        view.expanded.insert("work-0".into());
                        cx.notify();
                    });
                    let _ = window.draw(cx);
                })
                .unwrap();
                let path = format!("/tmp/grove-transcript-activity-{kind}-{width:.0}.png");
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(&path)
                    .unwrap();
                eprintln!("Native transcript activity fixture: {path}");
                cx.update_window(window.into(), |_, window, cx| {
                    capture_view.as_ref().unwrap().update(cx, |view, cx| {
                        view.session.update(cx, |session, cx| {
                            session.state.saved.items.truncate(1);
                            session.state.saved.items.push(json!({
                                "id":"child-started", "type":"subAgentActivity", "kind":"started",
                                "agentThreadId":"child-fixture", "agentPath":"root/reviewer"
                            }));
                            cx.notify();
                        });
                    });
                    let _ = window.draw(cx);
                })
                .unwrap();
                cx.run_until_parked();
                let path = format!("/tmp/grove-transcript-empty-work-{kind}-{width:.0}.png");
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(&path)
                    .unwrap();
                eprintln!("Native empty work fixture: {path}");
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
    }

    /// Opt-in native Metal rendering without modifying the installed app.
    #[test]
    #[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
    #[cfg(target_os = "macos")]
    fn selection_reference_native_snapshots() {
        use gpui::AppContext as _;
        let mut cx = gpui::HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            Arc::new(crate::assets::Assets),
            gpui_platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_reduce_motion(true);
        });
        let previous_theme = grove_core::theme::current().name.clone();
        for appearance in ["dark", "light"] {
            cx.update(|cx| {
                assert!(grove_core::theme::set_by_name(&format!(
                    "grove-{appearance}"
                )));
                c::sync_component_theme(cx);
            });
            for width in [1024.0, 400.0] {
                let quote = codex::ReferenceDraft {
                    id: "native-reference".into(),
                    thread_id: Some("thread-ui".into()),
                    sources: vec![codex::ReferenceSource {
                        message_id: "answer".into(),
                        role: "Assistant".into(),
                    }],
                    text: "Drafts survive reconnection.".into(),
                };
                let mut state = snapshot();
                state.saved.draft = "Can you explain how this works?".into();
                state.saved.draft_reference = Some(quote.clone());
                state.saved.items = vec![
                    json!({"id":"question", "type":"userMessage", "content":[{"type":"text", "text":"What happens if the agent disconnects while I’m writing?"}]}),
                    json!({"id":"answer", "type":"agentMessage", "text":"Your conversation stays open while Grove reconnects to the agent.\n\nYour message and attached images stay in the composer. Drafts survive reconnection.\n\nYou can continue editing your message while you wait."}),
                ];
                let (handle, _actions) = Handle::channel();
                let (_updates, receive) = mpsc::channel();
                let connection = Connection {
                    handle,
                    updates: receive.into(),
                };
                let retained = Rc::new(std::cell::RefCell::new(None));
                let captured = retained.clone();
                let window = cx
                    .open_window(
                        gpui::size(gpui::px(width), gpui::px(800.0)),
                        move |window, cx| {
                            let session =
                                cx.new(|cx| CodexSession::from_connection(state, connection, cx));
                            let chat = cx.new(|cx| CodexView::new(session, window, cx));
                            *captured.borrow_mut() = Some(chat.clone());
                            cx.new(|cx| gpui_component::Root::new(chat, window, cx).bordered(false))
                        },
                    )
                    .unwrap();
                // Native text shaping and Input auto-grow publish layout after
                // the initial frame. Drive those actual frames before capture.
                for _ in 0..12 {
                    cx.advance_clock(std::time::Duration::from_millis(16));
                    cx.run_until_parked();
                    cx.update_window(window.into(), |_, window, cx| {
                        window.simulate_next_frame(cx);
                        let _ = window.draw(cx);
                    })
                    .unwrap();
                }
                cx.update_window(window.into(), |_, window, cx| {
                    let _ = window.draw(cx);
                    let chat = retained.borrow();
                    let view = chat.as_ref().unwrap().read(cx);
                    assert_eq!(
                        view.input.read(cx).value().as_ref(),
                        "Can you explain how this works?"
                    );
                    assert_eq!(view.reference.as_ref().unwrap().text, quote.text);
                    let input = view.input.read(cx).range_to_bounds(&(0..1)).expect("native input text shaped");
                    let pane = view.transcript_bounds.get();
                    let viewport = window.viewport_size();
                    eprintln!("Native quote layout {width}/{appearance}: transcript={pane:?}, input={input:?}, viewport={viewport:?}");
                    assert!(input.top() >= gpui::px(0.) && input.bottom() <= viewport.height, "composer input outside screenshot viewport");
                    assert!(pane.bottom() < input.top() && pane.bottom() < viewport.height - gpui::px(100.), "quote/composer must reserve visible space below transcript");
                    assert!(view
                        .selection_views
                        .values()
                        .all(|owner| !owner.read(cx).source().contains("\"message_id\":")));
                })
                .unwrap();
                let path = format!("/tmp/grove-selection-reply-{width:.0}-{appearance}.png");
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(&path)
                    .unwrap();
                eprintln!("Native selection reference fixture: {path}");
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
        cx.update(|cx| {
            assert!(grove_core::theme::set_by_name(&previous_theme));
            c::sync_component_theme(cx);
        });
    }

    fn draw(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    #[gpui::test]
    fn transcript_cache_reuses_frames_and_refreshes_notified_session_edits(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, cx| {
                        session.state.saved.provider = provider;
                        session.state.saved.items =
                            vec![json!({"id":"answer", "type":"agentMessage", "text":"one"})];
                        cx.notify();
                    });
                });
            });
            draw(cx);
            let before = view.read_with(cx, |view, _| view.transcript_cache.clone().unwrap());
            cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
            draw(cx);
            view.read_with(cx, |view, _| {
                assert!(Rc::ptr_eq(&before, view.transcript_cache.as_ref().unwrap()));
            });
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, cx| {
                        session.state.saved.items[0]["text"] = json!("two");
                        session.state.error = Some("new error".into());
                        cx.notify();
                    });
                });
            });
            draw(cx);
            view.read_with(cx, |view, _| {
                let after = view.transcript_cache.as_ref().unwrap();
                assert_ne!(before.content, after.content);
                assert_eq!(after.state.saved.items[0]["text"], "two");
                assert_eq!(after.state.error.as_deref(), Some("new error"));
                assert_eq!(after.response_groups[0].copy_text, "two");
            });
        }
    }

    #[gpui::test]
    fn transcript_cache_sees_local_error_clear_before_observer_delivery(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _actions, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, _| {
                    session.state.error = Some("old error".into());
                });
                let _ = view.render(window, cx);
                let before = view.transcript_cache.as_ref().unwrap().source.clone();
                assert_eq!(before.error.as_deref(), Some("old error"));
                view.session.update(cx, |session, cx| {
                    // Successful sends do not notify the entity; this mirrors local
                    // error clearing before a provider has published a new snapshot.
                    session.state.error = None;
                    let forked = session.state.shared();
                    assert!(!Rc::ptr_eq(&before, &forked));
                    let forked_address = Rc::as_ptr(&forked);
                    drop(forked);
                    session.state.saved.draft = "local draft".into();
                    assert_eq!(forked_address, Rc::as_ptr(&session.state.shared()));
                    session.send(Action::Draft("local draft".into()), cx);
                });
                let _ = view.render(window, cx);
                let after = view.transcript_cache.as_ref().unwrap();
                assert!(after.state.error.is_none());
                assert_eq!(after.state.saved.draft, "local draft");
                assert_eq!(before.error.as_deref(), Some("old error"));
            });
        });
    }

    #[gpui::test]
    fn transcript_motion_coalesces_display_frames_and_stops_with_reduced_motion(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Working;
                    session.state.saved.items =
                        vec![json!({"id":"answer", "type":"agentMessage", "text":"old"})];
                    cx.notify();
                });
            });
        });
        draw(cx);
        cx.update(gpui::Window::simulate_next_frame);
        draw(cx);
        let notifications = Rc::new(Cell::new(0));
        let observed = notifications.clone();
        let watch = cx.update(|_, cx| {
            cx.observe(&view, move |_, _| {
                observed.set(observed.get() + 1);
            })
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, _| {
                    session.state.saved.items[0]["text"] = json!(format!("old{}", "界".repeat(80)));
                });
                // Follow and reveal both need a frame. Repeated renders must share it.
                let _ = view.render(window, cx);
                let _ = view.render(window, cx);
                assert!(view.smooth_follow);
                assert!(view.stream_text["answer"].1 < view.stream_text["answer"].0.len());
                assert!(view.motion_frame_pending.get());
            });
        });
        notifications.set(0);
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            assert!(!view.read(cx).motion_frame_pending.get());
        });
        // Observe this one display callback before pumping the executor, which
        // deliberately drains every subsequent animation frame in GPUI tests.
        assert_eq!(
            notifications.get(),
            1,
            "follow and reveal share one notification"
        );
        cx.update(|_, cx| cx.set_reduce_motion(true));
        draw(cx);
        // Any already queued final follow frame settles with reduced motion.
        cx.update(gpui::Window::simulate_next_frame);
        draw(cx);
        notifications.set(0);
        for _ in 0..3 {
            cx.update(gpui::Window::simulate_next_frame);
            draw(cx);
        }
        assert_eq!(notifications.get(), 0);
        view.read_with(cx, |view, _| {
            assert!(!view.motion_frame_pending.get());
            assert_eq!(
                view.stream_text["answer"].1,
                view.stream_text["answer"].0.len()
            );
        });
        drop(watch);
    }

    #[test]
    fn transcript_cache_tracks_optimistic_acknowledgement_and_thread_changes() {
        let source = Rc::new(snapshot());
        let optimistic = vec![json!({"id":"pending", "type":"userMessage", "text":"next"})];
        let cache = TranscriptRenderCache::new(source.clone(), optimistic.clone());
        assert!(cache.matches(&source, &optimistic));
        assert!(!cache.matches(&source, &[])); // rejected or removed bubble
        assert!(source.saved.items.is_empty());
        assert_eq!(cache.state.saved.items.len(), 1);
        let mut acknowledged = source.as_ref().clone();
        acknowledged.saved.items = optimistic.clone();
        let acknowledged = Rc::new(acknowledged);
        assert!(!cache.matches(&acknowledged, &optimistic));
        let acknowledged_cache = TranscriptRenderCache::new(acknowledged.clone(), vec![]);
        assert_eq!(acknowledged_cache.state.saved.items.len(), 1);
        assert_eq!(cache.content, acknowledged_cache.content);
        let mut changed_thread = acknowledged.as_ref().clone();
        changed_thread.saved.thread_id = Some("other-thread".into());
        let changed_thread = TranscriptRenderCache::new(Rc::new(changed_thread), vec![]);
        assert_ne!(acknowledged_cache.content, changed_thread.content);
    }

    /// Measures preparation only, excluding GPUI element construction/layout/paint.
    /// Run explicitly with --ignored --nocapture; timings are not FPS evidence.
    #[test]
    #[ignore = "manual transcript preparation benchmark"]
    fn transcript_preparation_benchmark() {
        use std::{hint::black_box, time::Instant};
        let mut state = snapshot();
        state.saved.items = (0..2000)
            .map(|index| {
                json!({
                    "id":format!("message-{index}"),
                    "type":if index % 2 == 0 { "userMessage" } else { "agentMessage" },
                    "text":"Representative transcript content with code and prose. ".repeat(20)
                })
            })
            .collect();
        let source = Rc::new(state);
        let cache = Rc::new(TranscriptRenderCache::new(source.clone(), vec![]));
        // Warm up both paths, then alternate their order across nine samples.
        for _ in 0..3 {
            black_box(TranscriptRenderCache::new(
                Rc::new(source.as_ref().clone()),
                vec![],
            ));
            black_box(cache.matches(&source, &[]));
        }
        for sample in 0..9 {
            let baseline = || {
                let start = Instant::now();
                for _ in 0..100 {
                    let prepared =
                        TranscriptRenderCache::new(Rc::new(source.as_ref().clone()), vec![]);
                    assert_eq!(prepared.content, cache.content);
                    assert_eq!(prepared.task_key, cache.task_key);
                    assert_eq!(prepared.minimap_entries, cache.minimap_entries);
                    assert_eq!(prepared.state.saved.items.len(), 2000);
                    black_box(prepared);
                }
                start.elapsed().as_secs_f64() * 1000.0 / 100.0
            };
            let cached = || {
                let start = Instant::now();
                for _ in 0..100 {
                    let source = black_box(source.clone());
                    assert!(black_box(cache.matches(&source, &[])));
                    black_box(cache.clone());
                }
                start.elapsed().as_secs_f64() * 1000.0 / 100.0
            };
            let (old, new) = if sample % 2 == 0 {
                (baseline(), cached())
            } else {
                let new = cached();
                (baseline(), new)
            };
            println!("sample={sample} preparation_ms={old:.6} cached_ms={new:.6}");
        }
    }

    #[gpui::test]
    fn edit_confirmation_blocks_background_and_preserves_editor_and_composer(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        cx.simulate_resize(gpui::size(gpui::px(1000.0), gpui::px(1000.0)));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| input.set_value("Unrelated composer draft", window, cx));
                view.session.update(cx, |session, cx| {
                    session.state.edit_supported = true;
                    session.state.saved.draft = "Unrelated composer draft".into();
                    session.state.saved.items = vec![
                        json!({"id":"original-user", "type":"userMessage", "content":[{"type":"text", "text":"Original sent request"}]}),
                        json!({"id":"original-answer", "type":"agentMessage", "text":"Original answer"}),
                    ];
                    session.state.saved.edit_checkpoints.push(codex::EditCheckpoint {
                        start_item: 0,
                        turn_id: Some("original-turn".into()),
                        // Eligibility needs the checkpoint identity only; this test
                        // never confirms restoration or accesses the filesystem.
                        files: serde_json::from_value(json!({"roots":[]})).unwrap(),
                    });
                    cx.notify();
                });
            });
        });
        draw(cx);
        let edit = cx.debug_bounds("codex-edit-message-0").unwrap();
        cx.simulate_click(edit.center(), gpui::Modifiers::default());
        draw(cx);
        let editor = cx.debug_bounds("codex-inline-editor").unwrap();
        let message = cx.debug_bounds("codex-message-anchor-0").unwrap();
        let composer = cx.debug_bounds("codex-composer").unwrap();
        assert!(editor.top() >= message.top());
        assert!(editor.bottom() <= message.bottom());
        assert!(editor.bottom() <= composer.top());
        assert!(cx.debug_bounds("codex-message-body-0").is_none());
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.message_editor
                    .as_ref()
                    .unwrap()
                    .input
                    .update(cx, |input, cx| {
                        input.set_value("Edited inline request", window, cx);
                    });
            });
        });
        cx.simulate_keystrokes("enter");
        draw(cx);
        let confirmation = cx.debug_bounds("codex-edit-confirmation").unwrap();
        let source = cx.debug_bounds("codex-send-message-edit").unwrap();
        let overlay = cx.debug_bounds("codex-edit-confirmation-overlay").unwrap();
        assert!(
            (confirmation.right() - source.right()).abs() < gpui::px(1.),
            "tray={confirmation:?}, source={source:?}"
        );
        assert!(
            (confirmation.top() - source.bottom()).abs() < gpui::px(1.)
                || (confirmation.bottom() - source.top()).abs() < gpui::px(1.)
        );
        assert!(confirmation.top() >= overlay.top());
        assert!(confirmation.bottom() <= overlay.bottom());
        // Resizing moves the initiating action; attachment must follow this
        // frame's source instead of a captured window position or pane bottom.
        cx.simulate_resize(gpui::size(gpui::px(420.), gpui::px(700.)));
        draw(cx);
        let moved_source = cx.debug_bounds("codex-send-message-edit").unwrap();
        let moved_confirmation = cx.debug_bounds("codex-edit-confirmation").unwrap();
        assert!((moved_source.right() - source.right()).abs() > gpui::px(100.));
        assert!((moved_confirmation.right() - moved_source.right()).abs() < gpui::px(1.));
        assert!(
            (moved_confirmation.top() - moved_source.bottom()).abs() < gpui::px(1.)
                || (moved_confirmation.bottom() - moved_source.top()).abs() < gpui::px(1.)
        );
        assert!(moved_confirmation.left() >= gpui::px(0.));
        // A source scrolled entirely out of view is revealed before its tray
        // is painted; it never borrows a previous source's cached position.
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.smooth_follow = false;
                view.session.update(cx, |session, cx| {
                    session.state.saved.items[1]["text"] =
                        json!("Long reply below the edited source.\n\n".repeat(50));
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(
            !view.read_with(cx, |view, _| view.smooth_follow),
            "content updates cannot restart following behind a source-owned confirmation"
        );
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.scroll.scroll_to_bottom();
                cx.notify();
            });
        });
        draw(cx);
        // Unlike a platform window, this fixture has no automatic display-frame
        // loop. Deliver the scheduled ScrollAnchor and notification callbacks.
        for _ in 0..3 {
            if cx.debug_bounds("codex-edit-confirmation").is_some() {
                break;
            }
            cx.update(gpui::Window::simulate_next_frame);
            draw(cx);
        }
        let revealed_source = cx.debug_bounds("codex-send-message-edit").unwrap();
        let revealed_tray = cx.debug_bounds("codex-edit-confirmation").unwrap();
        let viewport = view.read_with(cx, |view, _| view.scroll.bounds());
        assert!(revealed_source.intersects(&viewport));
        assert!((revealed_tray.right() - revealed_source.right()).abs() < gpui::px(1.));
        assert!(
            (revealed_tray.top() - revealed_source.bottom()).abs() < gpui::px(1.)
                || (revealed_tray.bottom() - revealed_source.top()).abs() < gpui::px(1.)
        );
        // A pointer click on the veiled source cannot return ownership to an
        // editor behind the dialog, and Tab stays on the two dialog actions.
        let composer = cx.debug_bounds("codex-composer").unwrap();
        cx.simulate_click(composer.center(), gpui::Modifiers::default());
        cx.simulate_keystrokes("tab");
        view.read_with(cx, |view, _| {
            assert!(view.edit_confirmation.is_some());
        });
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.input.read(cx).value().as_ref(),
                "Unrelated composer draft"
            );
            assert_eq!(
                view.session.read(cx).state.saved.draft,
                "Unrelated composer draft"
            );
            assert_eq!(
                view.message_editor
                    .as_ref()
                    .unwrap()
                    .input
                    .read(cx)
                    .value()
                    .as_ref(),
                "Edited inline request"
            );
        });
        cx.update(|window, cx| {
            view.read_with(cx, |view, _| {
                assert!(view.edit_confirm_focus.is_focused(window));
            });
        });
        let cancel = cx.debug_bounds("codex-cancel-edit").unwrap();
        cx.simulate_click(cancel.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("codex-edit-confirmation").is_none());
        assert!(cx.debug_bounds("codex-inline-editor").is_some());
        cx.update(|window, cx| {
            view.read_with(cx, |view, cx| {
                assert!(view
                    .message_editor
                    .as_ref()
                    .unwrap()
                    .input
                    .focus_handle(cx)
                    .is_focused(window));
            });
        });
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(cx.debug_bounds("codex-inline-editor").is_none());
        assert!(cx.debug_bounds("codex-message-body-0").is_some());
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.input.read(cx).value().as_ref(),
                "Unrelated composer draft"
            );
            assert_eq!(
                view.session.read(cx).state.saved.items[0]["content"][0]["text"],
                "Original sent request"
            );
            assert_eq!(view.session.read(cx).state.saved.items.len(), 2);
        });
        assert!(!actions.try_iter().any(|action| matches!(
            action,
            Action::EditMessage { .. }
                | Action::EditMessageInline { .. }
                | Action::PromptInline { .. }
        )));
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
    fn claude_starting_shows_composer_and_queues_images_without_hiding_history(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("While connecting", window, cx);
                });
                view.images = vec!["data:image/png;base64,aGVsbG8=".into()];
                view.session.update(cx, |session, cx| {
                    session.state.saved.provider = codex::Provider::Claude;
                    session.state.saved.thread_id = None;
                    session.state.status = Status::Starting;
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-provider-connection").is_none());
        assert!(cx.debug_bounds("codex-composer").is_some());
        assert!(cx.debug_bounds("codex-empty").is_some());
        view.read_with(cx, |view, cx| {
            assert!(!view.draft_action_pending(cx));
            assert_eq!(
                live_activity(&view.session.read(cx).state).as_deref(),
                Some("Connecting to Claude Code…")
            );
        });
        cx.update(|window, cx| view.update(cx, |view, cx| view.submit(window, cx)));
        assert!(actions.try_iter().any(|action| matches!(action, Action::SubmitWithMode { text, images, mode: SubmissionMode::Queue, .. } if text == "While connecting" && images == vec!["data:image/png;base64,aGVsbG8="])));
        view.update(cx, |view, cx| view.session.update(cx, |session, cx| {
            session.state.saved.items.push(json!({"type":"userMessage", "id":"earlier", "content":[{"type":"text","text":"Earlier request"}]}));
            cx.notify();
        }));
        draw(cx);
        assert!(cx.debug_bounds("codex-message-body-0").is_some());
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
        let sent = actions.try_iter().collect::<Vec<_>>();
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
    fn queued_resume_is_keyboard_accessible_when_idle_and_absent_while_busy(
        cx: &mut gpui::TestAppContext,
    ) {
        fn tab_reaches_resume(view: &Entity<CodexView>, cx: &mut gpui::VisualTestContext) -> bool {
            cx.update(|window, _| window.blur());
            for _ in 0..32 {
                // Shell routes unconsumed Tab to this production GPUI traversal
                // boundary; the standalone conversation fixture has no Shell.
                cx.update(gpui::Window::focus_next);
                draw(cx);
                if cx.update(|window, cx| {
                    view.read_with(cx, |view, _| view.queue_resume_focus.is_focused(window))
                }) {
                    return true;
                }
            }
            false
        }
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Idle;
                    session.state.queued_prompts.push(codex::QueuedPrompt {
                        id: 1,
                        text: "Queued request".into(),
                        images: vec![],
                    });
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-resume-queue").is_some());
        assert!(
            tab_reaches_resume(&view, cx),
            "Tab traversal reaches the idle Send next control"
        );
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(actions
            .try_iter()
            .any(|action| matches!(action, Action::ResumeQueue)));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Working;
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-resume-queue").is_none());
        cx.simulate_keystrokes("space");
        draw(cx);
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::ResumeQueue)));
        for (status, uncertain) in [(Status::Disconnected, false), (Status::Idle, true)] {
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, cx| {
                        session.state.status = status;
                        session.state.queue_uncertain = if uncertain { vec![1] } else { vec![] };
                        cx.notify();
                    });
                });
            });
            draw(cx);
            assert!(
                !tab_reaches_resume(&view, cx),
                "unavailable queue actions are outside Tab order"
            );
            if let Some(control) = cx.debug_bounds("codex-resume-queue") {
                cx.simulate_click(control.center(), gpui::Modifiers::default());
                draw(cx);
            }
            assert!(!actions
                .try_iter()
                .any(|action| matches!(action, Action::ResumeQueue)));
        }
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
    fn provider_picker_dashboard_entry_commit_and_escape_preserve_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(crate::settings::SettingsState::new(
                grove_core::storage::Store::default(),
            ));
        });
        let reference = codex::ReferenceDraft {
            id: "picker-reference".into(),
            thread_id: None,
            sources: Vec::new(),
            text: "Keep this quote".into(),
        };
        let mut state = snapshot();
        state.saved.thread_id = None;
        state.saved.cwd = "/missing-grove-picker-fixture".into();
        state.saved.draft = "Keep the prompt".into();
        state.saved.draft_images = vec!["attachment-fixture".into()];
        state.saved.draft_reference = Some(reference.clone());
        let mut retained = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let session = cx.new(|cx| CodexSession::draft(state.saved, Vec::new(), cx));
            session.update(cx, |session, _| {
                session.state.models = vec![json!({"model":"code-1","displayName":"Code model"})];
                session.state.model = Some("code-1".into());
            });
            let tray = cx.new(|_| {
                super::super::sidebar::dashboard::DestinationTray::picker_test_tray(
                    "/missing-grove-picker-fixture".into(),
                )
            });
            let chat = cx.new(|cx| {
                let mut view = CodexView::new(session, window, cx);
                view.attach_dashboard_tray(tray, cx);
                view
            });
            retained = Some(chat.clone());
            gpui_component::Root::new(chat, window, cx).bordered(false)
        });
        let view = retained.unwrap();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.open_menu(ComposerMenu::Settings, window, cx);
            });
        });
        draw(cx);
        let model = cx.debug_bounds("codex-card-model-label").unwrap().center();
        cx.simulate_click(model, gpui::Modifiers::default());
        draw(cx);
        cx.update(|_, cx| {
            view.read(cx)
                .model_picker
                .clone()
                .unwrap()
                .update(cx, |picker, cx| {
                    picker.seed_test_catalogs();
                    cx.notify();
                });
        });
        draw(cx);
        let rail = cx.debug_bounds("model-provider-2").unwrap().center();
        cx.simulate_click(rail, gpui::Modifiers::default());
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.session.read(cx).state.saved.provider,
                codex::Provider::Codex
            );
        });
        let row = cx.debug_bounds("model-picker-row-0").unwrap();
        cx.simulate_click(
            gpui::point(row.left() + gpui::px(35.), row.center().y),
            gpui::Modifiers::default(),
        );
        draw(cx);
        view.read_with(cx, |view, cx| {
            let state = &view.session.read(cx).state;
            assert_eq!(state.saved.provider, codex::Provider::Claude);
            assert_eq!(state.saved.composer.model.as_deref(), Some("claude-1"));
            assert!(view.menu == Some(ComposerMenu::Settings));
            assert_eq!(view.input.read(cx).value().as_ref(), "Keep the prompt");
            assert_eq!(state.saved.draft_images, ["attachment-fixture"]);
            assert_eq!(state.saved.draft_reference.as_ref(), Some(&reference));
            assert_eq!(state.saved.cwd, "/missing-grove-picker-fixture");
            assert!(view.session.read(cx).is_deferred());
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.model_from_card = true;
                view.open_menu(ComposerMenu::Model, window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("escape");
        draw(cx);
        view.read_with(cx, |view, _| {
            assert!(view.menu == Some(ComposerMenu::Settings));
        });
        assert!(cx.update(|window, cx| view.read(cx).card_model_focus.is_focused(window)));
    }

    #[gpui::test]
    fn composer_choices_dispatch_configuration_and_commands_without_prompts(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.open_menu(ComposerMenu::Model, window, cx);
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
                    optimistic: None,
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
                    optimistic: None,
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
    fn steering_bubble_survives_early_echo_until_ack_and_recovers_rejection(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, _| {
                    session.state.status = Status::Working;
                    session.state.turn_id = Some("turn".into());
                });
                view.input
                    .update(cx, |input, cx| input.set_value("steer me", window, cx));
                view.submit_mode(SubmissionMode::Steer, window, cx);
                assert_eq!(view.bubble_entrances.len(), 1);
                assert_eq!(
                    view.bubble_entrances[0].optimistic.as_ref().unwrap()["localSteering"],
                    true
                );
                assert!(view.session.read(cx).state.saved.items.is_empty());
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-message-row-0").is_some());
        let id = actions
            .try_iter()
            .find_map(|action| match action {
                Action::SubmitWithMode {
                    id,
                    mode: SubmissionMode::Steer,
                    ..
                } => Some(id),
                _ => None,
            })
            .unwrap();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                let mut state = view.session.read(cx).state.clone();
                state.saved.items.push(
                    json!({"id":"echo", "type":"userMessage", "content":[{"text":"steer me"}]}),
                );
                view.sync_bubble_entrances(&state, true, cx.background_executor().now());
                let bubble = view.bubble_entrances[0].optimistic.as_ref().unwrap();
                assert_eq!(bubble["id"], "echo");
                assert_eq!(bubble["localSteering"], true);
                state.accepted_submission = Some(id);
                view.sync_bubble_entrances(&state, true, cx.background_executor().now());
                assert!(view.bubble_entrances.is_empty());
                view.record_submission_bubble(id + 1, "retry".into(), vec![], true, false, cx);
                state.rejected_submission = Some(id + 1);
                view.sync_bubble_entrances(&state, true, cx.background_executor().now());
                assert!(view.bubble_entrances.is_empty());
            });
        });
    }

    #[gpui::test]
    fn queued_steering_projects_once_and_keeps_echo_gap_visible(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, _| {
                    session.state.status = Status::Working;
                    session.state.turn_id = Some("turn".into());
                    session.state.accepted_submission = Some(42);
                    session.state.queued_prompts.push(codex::QueuedPrompt {
                        id: 42,
                        text: "queued steer".into(),
                        images: vec![],
                    });
                });
                assert!(view.queue_action(Action::SteerQueued(42), cx));
                let mut state = view.session.read(cx).state.clone();
                view.sync_bubble_entrances(&state, true, cx.background_executor().now());
                assert_eq!(
                    view.bubble_entrances[0].optimistic.as_ref().unwrap()["localSteering"],
                    true
                );
                state.queued_prompts.clear();
                view.sync_bubble_entrances(&state, true, cx.background_executor().now());
                assert_eq!(
                    view.bubble_entrances[0].optimistic.as_ref().unwrap()["localSteering"],
                    false
                );
                state.saved.items.push(
                    json!({"id":"echo", "type":"userMessage", "content":[{"text":"queued steer"}]}),
                );
                view.sync_bubble_entrances(&state, true, cx.background_executor().now());
                assert!(view.bubble_entrances.is_empty());
            });
        });
        assert!(actions
            .try_iter()
            .any(|action| matches!(action, Action::SteerQueued(42))));
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
        // The provider has not received or acknowledged the action yet.
        let entering = cx.debug_bounds("codex-message-row-0").unwrap();
        let animation_start = view.read_with(cx, |view, _| {
            view.animating_bubbles.values().next().unwrap().0
        });
        assert!(view.read_with(cx, |view, cx| {
            view.session.read(cx).state.saved.items.is_empty()
        }));
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
        assert!(cx.debug_bounds("codex-message-row-0").is_some());
        assert!(cx.debug_bounds("codex-message-row-1").is_none());
        assert_eq!(
            view.read_with(cx, |view, _| view.animating_bubbles["fresh"].0),
            animation_start
        );
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
    fn image_only_send_is_visible_and_reconciles_after_identical_history(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, updates) = fixture(cx);
        let image = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jM1sAAAAASUVORK5CYII=";
        let mut state = snapshot();
        state.saved.items.push(json!({"id":"old-image", "type":"userMessage", "content":[{"type":"image", "url":image}]}));
        updates.send(state.clone()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.images = vec![image.into()];
                view.submit(window, cx);
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-message-row-1").is_some());
        assert!(cx.debug_bounds("codex-message-images-1").is_some());
        let submission = actions
            .try_iter()
            .find_map(|action| match action {
                Action::Submit { id, .. } => Some(id),
                _ => None,
            })
            .unwrap();
        state.accepted_submission = Some(submission);
        state.status = Status::Working;
        updates.send(state.clone()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(cx.debug_bounds("codex-message-images-1").is_some());
        state.saved.items.push(json!({"id":"other-image", "type":"userMessage", "content":[{"type":"image", "url":"different-image"}]}));
        updates.send(state.clone()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(cx.debug_bounds("codex-message-images-2").is_some());
        assert_eq!(view.read_with(cx, |view, _| view.bubble_entrances.len()), 1);
        state.saved.items.push(json!({"id":"new-image", "type":"userMessage", "content":[{"type":"image", "url":image}]}));
        updates.send(state).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(cx.debug_bounds("codex-message-images-2").is_some());
        assert!(cx.debug_bounds("codex-message-row-3").is_none());
        assert!(view.read_with(cx, |view, _| view.bubble_entrances.is_empty()));
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
    fn live_activity_avoids_duplicate_work_and_aligns_meaningful_status(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_view, cx, _, updates) = fixture(cx);
        let mut state = snapshot();
        state.status = Status::Working;
        state.saved.items.clear();
        state.saved.thread_id = None;
        assert_eq!(
            live_activity(&state).as_deref(),
            Some("Connecting to Codex…")
        );
        state.saved.items = vec![
            json!({"id":"prompt","type":"userMessage","content":[{"text":"Check this"}]}),
            json!({"id":"reply","type":"agentMessage","phase":"commentary","text":"Checking this."}),
        ];
        assert!(work_header_at(&state, 1).unwrap().2);
        assert_eq!(live_activity(&state), None);
        updates.send(state.clone()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(cx.debug_bounds("codex-live-activity").is_none());
        state.compacting = true;
        assert_eq!(
            live_activity(&state).as_deref(),
            Some("Compacting context…")
        );
        updates.send(state.clone()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        for width in [1000.0, 400.0] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.0)));
            draw(cx);
            let status = cx.debug_bounds("codex-live-activity-text").unwrap();
            let body = cx.debug_bounds("codex-message-body-1").unwrap();
            assert_eq!(status.left(), body.left(), "status alignment at {width}");
        }
        state.compacting = false;
        state.approvals.push(json!({"id":"permission"}));
        assert_eq!(
            live_activity(&state).as_deref(),
            Some("Waiting for your permission…")
        );
        state.approvals.clear();
        state.elicitations.push(ElicitationRequest::parse_codex(json!("answer"), &json!({"message":"Name", "requestedSchema":{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}})).unwrap());
        assert_eq!(
            live_activity(&state).as_deref(),
            Some("Waiting for your answers…")
        );
    }

    #[gpui::test]
    fn manual_compaction_preserves_completed_work_disclosures(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, updates) = fixture(cx);
        let mut state = snapshot();
        state.status = Status::Done;
        state.saved.items = vec![
            json!({"id":"prompt","type":"userMessage","content":[{"text":"Check status"}]}),
            json!({"id":"comment","type":"agentMessage","phase":"commentary","text":"Checking status"}),
            json!({"id":"reason","type":"reasoning","text":"Inspect the checkout"}),
            json!({"id":"command","type":"commandExecution","command":"pwd","status":"completed"}),
            json!({"id":"answer","type":"agentMessage","phase":"final_answer","text":"Done"}),
        ];
        // Imported history has no recorded work periods. A new compaction turn
        // must not make that completed history part of the live turn.
        assert!(state.saved.work_periods.is_empty());
        updates.send(state.clone()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(cx.debug_bounds("codex-message-body-1").is_none());
        assert!(cx.debug_bounds("codex-toggle-tool-2").is_none());
        assert!(cx.debug_bounds("codex-toggle-tool-3").is_none());
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input
                    .update(cx, |input, cx| input.set_value("/compact", window, cx));
                view.submit(window, cx);
            });
        });
        assert!(actions
            .try_iter()
            .any(|action| matches!(action, Action::Compact)));
        state.apply(&json!({"method":"turn/started","params":{"threadId":state.saved.thread_id,"turn":{"id":"compact-turn"}}}));
        for method in ["item/started", "item/completed"] {
            state.apply(&json!({"method":method,"params":{"threadId":state.saved.thread_id,"turnId":"compact-turn","item":{"id":"compact","type":codex::COMPACTION_ITEM}}}));
            updates.send(state.clone()).unwrap();
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(40));
            draw(cx);
            assert!(
                cx.debug_bounds("codex-message-body-1").is_none(),
                "{method}"
            );
            assert!(cx.debug_bounds("codex-toggle-tool-2").is_none(), "{method}");
            assert!(cx.debug_bounds("codex-toggle-tool-3").is_none(), "{method}");
            assert!(cx.debug_bounds("codex-message-body-4").is_some());
        }
        state.apply(&json!({"method":"turn/completed","params":{"threadId":state.saved.thread_id,"turn":{"id":"compact-turn","status":"completed"}}}));
        updates.send(state).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(cx.debug_bounds("codex-toggle-tool-2").is_none());
        assert!(cx.debug_bounds("codex-toggle-tool-3").is_none());
        // The disclosure still opens normally after compaction.
        let header = cx.debug_bounds("codex-work-header-1").unwrap();
        cx.simulate_click(header.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("codex-message-body-1").is_some());
        assert!(cx.debug_bounds("codex-toggle-tool-2").is_some());
        assert!(cx.debug_bounds("codex-toggle-tool-3").is_some());
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

    #[test]
    fn completion_trigger_steps_past_multibyte_whitespace() {
        // U+00A0 is whitespace but two bytes wide; the token starts after all of it.
        let text = "see\u{a0}@src next";
        let cursor = "see\u{a0}@src".len();
        assert_eq!(completion_trigger(text, cursor), Some((5..9, '@', "src")));
        let text = "Image 1\u{a0} We";
        assert_eq!(completion_trigger(text, "Image 1\u{a0}".len()), None);
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
        // Explicit expansion reveals earlier calls while the current turn is still running.
        assert!(cx.debug_bounds("codex-toggle-tool-4").is_some());
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

    #[test]
    fn redirect_controls_require_an_active_unblocked_turn_for_both_providers() {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let mut state = snapshot();
            state.saved.provider = provider;
            state.status = Status::Working;
            assert!(!active_turn_redirect_available(&state));
            state.turn_id = Some("active".into());
            if provider == codex::Provider::Claude {
                assert!(!active_turn_redirect_available(&state));
                state.can_redirect = true;
            }
            assert!(active_turn_redirect_available(&state));
            state.compacting = true;
            assert!(!active_turn_redirect_available(&state));
            state.compacting = false;
            state.status = Status::Starting;
            assert!(!active_turn_redirect_available(&state));
        }
    }

    #[gpui::test]
    fn claude_active_turn_redirect_dispatches_and_preserves_draft_until_ack(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.provider = codex::Provider::Claude;
                    session.state.status = Status::Working;
                    session.state.turn_id = Some("active".into());
                    session.state.can_redirect = true;
                    cx.notify();
                });
                view.input.update(cx, |input, cx| {
                    input.set_value("change direction", window, cx);
                });
                view.submit_mode(SubmissionMode::Steer, window, cx);
                assert_eq!(view.input.read(cx).value().as_ref(), "change direction");
            });
        });
        assert!(actions.try_iter().any(|action| matches!(action, Action::SubmitWithMode { mode: SubmissionMode::Steer, text, .. } if text == "change direction")));
    }

    #[gpui::test]
    fn claude_checklists_are_hidden_and_legacy_plan_keeps_tool_details(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.provider = codex::Provider::Claude;
                    session.state.saved.items = vec![
                        // A tool result with no structured list keeps today's rendering.
                        serde_json::json!({
                            "id":"claude-plan", "type":"plan", "tool":"TaskUpdate",
                            "text":"- [x] Verify parity", "status":"completed",
                            "arguments":{"taskId":"7","status":"completed"},
                            "result":{"content":[{"type":"text","text":"Updated task #7 status"}]}
                        }),
                        serde_json::json!({
                            "id":"claude-todos", "type":"plan", "tool":"TodoWrite",
                            "text":"- [x] Verify parity", "status":"completed",
                            "plan":[{"id":"todo-0","step":"Verify parity","status":"completed"}]
                        }),
                    ];
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-message-body-0").is_some());
        assert!(cx.debug_bounds("codex-plan-detail-0").is_none());
        let toggle = cx.debug_bounds("codex-toggle-details-0").unwrap();
        cx.simulate_click(toggle.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("codex-plan-detail-0").is_some());
        assert!(cx.debug_bounds("codex-message-body-0").is_some());
        assert!(view.read_with(cx, |view, _| view.expanded.contains("claude-plan")));
        cx.simulate_click(toggle.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("codex-plan-detail-0").is_none());
        // Structured checklist updates only appear in the docked task panel.
        assert!(cx.debug_bounds("codex-task-diff-1").is_none());
        assert!(cx.debug_bounds("codex-message-anchor-1").is_none());
        assert!(cx.debug_bounds("codex-task-strip").is_some());
        assert!(cx.debug_bounds("codex-message-body-1").is_none());
        assert!(cx.debug_bounds("codex-toggle-details-1").is_none());
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.provider = codex::Provider::Codex;
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-message-body-0").is_some());
        assert!(cx.debug_bounds("codex-toggle-details-0").is_none());
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
                })
                .collect(),
            explanation: None,
            item: 0,
        }
    }

    #[test]
    fn task_phase_maps_session_status() {
        for (status, phase) in [
            (Status::Starting, TaskPhase::Running),
            (Status::Working, TaskPhase::Running),
            (Status::Waiting, TaskPhase::Waiting),
            (Status::Interrupted, TaskPhase::Halted),
            (Status::Failed, TaskPhase::Halted),
            (Status::Disconnected, TaskPhase::Halted),
            (Status::Idle, TaskPhase::Stale),
            (Status::Done, TaskPhase::Stale),
        ] {
            assert_eq!(task_phase(status), phase, "{status:?}");
        }
        // Completed marks stay green even when the session is idle.
        assert_eq!(
            task_mark_phase(TaskStatus::Completed, TaskPhase::Stale),
            TaskPhase::Running
        );
        assert_eq!(
            task_mark_phase(TaskStatus::InProgress, TaskPhase::Halted),
            TaskPhase::Halted
        );
    }

    #[test]
    fn task_summary_reads_per_state() {
        use TaskStatus::{Completed, InProgress, Pending};
        let list = task_list(&[Completed, InProgress, Pending]);
        assert_eq!(task_header_status(&list), InProgress);
        assert_eq!(
            task_summary(&list, TaskPhase::Running).text(),
            "Doing step 1"
        );
        assert_eq!(
            task_summary(&list, TaskPhase::Waiting),
            TaskSummary::Waiting("Step 1".into())
        );
        assert_eq!(
            task_summary(&list, TaskPhase::Waiting).text(),
            "Step 1 · Waiting for you"
        );
        assert_eq!(
            task_summary(&list, TaskPhase::Halted).text(),
            "Stopped at Step 1"
        );
        assert_eq!(task_summary(&list, TaskPhase::Stale).text(), "Doing step 1");
        let done = task_list(&[Completed; 7]);
        assert_eq!(task_header_status(&done), Completed);
        for phase in [TaskPhase::Running, TaskPhase::Halted, TaskPhase::Waiting] {
            assert_eq!(task_summary(&done, phase).text(), "All 7 tasks done");
        }
        let pending = task_list(&[Completed, Pending]);
        assert_eq!(task_header_status(&pending), Pending);
        assert_eq!(
            task_summary(&pending, TaskPhase::Running),
            TaskSummary::None
        );
    }

    #[test]
    fn spinner_ticks_per_frame_inverts_the_animation_clock() {
        for frame in 0..crate::entities::animation_clock::SPINNER_FRAMES {
            assert_eq!(
                crate::entities::animation_clock::spinner_frame(frame * SPINNER_TICKS_PER_FRAME),
                frame
            );
        }
    }

    #[test]
    fn task_cache_ignores_streaming_text_but_tracks_checklists_and_new_items() {
        let mut items = vec![
            json!({"id":"u1", "type":"userMessage", "content":[{"text":"Go"}]}),
            json!({"id":"p1", "type":"plan", "text":"", "plan":[{"step":"A","status":"inProgress"}]}),
            json!({"id":"a1", "type":"agentMessage", "text":"Working"}),
        ];
        let key = task_cache_key(&items);
        items[2]["text"] = json!("Working on it");
        assert_eq!(task_cache_key(&items), key);
        items[1]["plan"][0]["status"] = json!("completed");
        let changed = task_cache_key(&items);
        assert_ne!(changed, key);
        items.push(json!({"id":"u2", "type":"userMessage", "content":[{"text":"Next"}]}));
        assert_ne!(task_cache_key(&items), changed);
        let cache = task_cache(&items, task_cache_key(&items));
        assert_eq!(cache.list.unwrap().done(), 1);
    }

    #[gpui::test]
    fn checklist_snapshots_are_hidden_and_proposed_plans_stay(cx: &mut gpui::TestAppContext) {
        let (view, cx, _, _) = fixture(cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.items = vec![
                        json!({"id":"u1", "type":"userMessage", "content":[{"text":"Go"}]}),
                        json!({"id":"grove-plan-t1", "type":"plan", "text":"- [ ] A", "plan":[
                            {"step":"A","status":"inProgress"},{"step":"B","status":"pending"}]}),
                        json!({"id":"proposed", "type":"plan", "text":"Proposed plan body"}),
                        // Identical snapshot: nothing changed, so nothing renders.
                        json!({"id":"grove-plan-t2", "type":"plan", "text":"- [ ] A", "plan":[
                            {"step":"A","status":"inProgress"},{"step":"B","status":"pending"}]}),
                    ];
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-task-diff-1").is_none());
        assert!(cx.debug_bounds("codex-message-anchor-1").is_none());
        assert!(cx.debug_bounds("codex-message-body-1").is_none());
        assert!(cx.debug_bounds("codex-message-body-2").is_some());
        assert!(cx.debug_bounds("codex-task-diff-3").is_none());
        assert!(cx.debug_bounds("codex-message-anchor-3").is_none());
        let list = view.read_with(cx, |view, _| {
            view.task_cache.as_ref().unwrap().list.clone().unwrap()
        });
        assert_eq!(list.total(), 2);
        assert_eq!(list.tasks[0].status, TaskStatus::InProgress);
        assert!(cx.debug_bounds("codex-task-strip").is_some());
    }

    #[gpui::test]
    fn session_task_strip_survives_a_clarification_answer(cx: &mut gpui::TestAppContext) {
        let (view, cx, _, _) = fixture(cx);
        cx.update(|_, cx| {
            cx.set_reduce_motion(true);
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Waiting;
                    session.state.saved.items = vec![
                        json!({"id":"p1", "type":"plan", "plan":[
                            {"step":"Inspect", "status":"completed"},
                            {"step":"Implement", "status":"inProgress"},
                            {"step":"Verify", "status":"pending"}]}),
                        json!({"id":"q1", "type":"agentMessage", "text":"Which option?"}),
                    ];
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-task-strip").is_some());
        let before = view.read_with(cx, |view, _| {
            view.task_cache.as_ref().unwrap().list.clone().unwrap()
        });
        assert_eq!((before.done(), before.total()), (1, 3));
        assert_eq!(before.now_label(), Some("Implement"));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Working;
                    session.state.saved.items.push(
                        json!({"id":"u2", "type":"userMessage", "content":[{"text":"Both equally"}]}),
                    );
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-task-strip").is_some());
        assert!(cx.debug_bounds("codex-task-row-1").is_some());
        assert!(cx.debug_bounds("codex-task-strip-stale").is_none());
        let after = view.read_with(cx, |view, _| {
            view.task_cache.as_ref().unwrap().list.clone().unwrap()
        });
        assert_eq!(after, before);
    }

    #[gpui::test]
    fn provider_notices_match_task_panel_and_recover_without_reconnect(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            for (width, rem) in [(1000.0, 16.0), (560.0, 16.0), (560.0, 20.0)] {
                cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(900.0)));
                cx.update(|window, cx| {
                    window.set_rem_size(gpui::px(rem));
                    view.update(cx, |view, cx| view.session.update(cx, |session, cx| {
                        session.state.saved.provider = provider;
                        session.state.status = Status::Failed;
                        session.state.error = Some("Upstream request failed.\n\nUnexpected <response>.".into());
                        session.state.notice = None;
                        session.state.saved.items = vec![
                            json!({"id":"u1","type":"userMessage","content":[{"text":"Go"}]}),
                            json!({"id":"plan","type":"plan","plan":[{"step":"Inspect","status":"completed"},{"step":"Build","status":"inProgress"}]}),
                        ];
                        cx.notify();
                    }));
                });
                draw(cx);
                let notice = cx.debug_bounds("codex-error").unwrap();
                let tasks = cx.debug_bounds("codex-task-strip-panel").unwrap();
                assert_eq!(
                    notice.left(),
                    tasks.left(),
                    "{provider:?} width={width} rem={rem}"
                );
                assert_eq!(
                    notice.right(),
                    tasks.right(),
                    "{provider:?} width={width} rem={rem}"
                );
                assert!(notice.bottom() <= tasks.top());
                assert!(cx.debug_bounds("codex-reconnect").is_none());
                let first = cx.debug_bounds("session-notice-body-line-0").unwrap();
                let second = cx.debug_bounds("session-notice-body-line-1").unwrap();
                assert!(
                    second.top() >= first.bottom(),
                    "provider line breaks remain separate native blocks"
                );
                let third = cx.debug_bounds("session-notice-body-line-2").unwrap();
                assert!(
                    third.top() >= second.bottom(),
                    "blank provider lines retain their height"
                );
                assert!(third.right() <= notice.right());
                let copy = cx.debug_bounds("session-notice-copy").unwrap();
                cx.simulate_click(copy.center(), gpui::Modifiers::default());
                cx.update(|_, cx| {
                    assert_eq!(
                        cx.read_from_clipboard().unwrap().text().as_deref(),
                        Some("Upstream request failed.\n\nUnexpected <response>.")
                    );
                });
            }
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    let body = "Connection interrupted".to_owned();
                    session.state.status = Status::Working;
                    session.state.error = Some(body.clone());
                    session.state.notice = Some(codex::SessionNotice {
                        kind: codex::SessionNoticeKind::Retrying,
                        provider: "fixture",
                        body,
                        reset_at: None,
                        usage_url: None,
                    });
                    cx.notify();
                });
            });
            draw(cx);
            assert!(cx.debug_bounds("codex-error").is_none());
            assert!(cx.debug_bounds("codex-live-activity").is_some());
            assert!(cx.debug_bounds("codex-reconnect").is_none());
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.error = None;
                    session.state.status = Status::Done;
                    cx.notify();
                });
            });
            draw(cx);
            assert!(cx.debug_bounds("codex-error").is_none());
            assert!(cx.debug_bounds("codex-live-activity").is_none());
        }
    }

    #[gpui::test]
    fn provider_warning_shares_transcript_rail_at_narrow_widths(cx: &mut gpui::TestAppContext) {
        let (view, cx, _, _) = fixture(cx);
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            for width in [1000.0, 560.0] {
                cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(900.0)));
                cx.update(|_, cx| view.update(cx, |view, cx| view.session.update(cx, |session, cx| {
                    session.state.saved.provider = provider;
                    session.state.status = Status::Done;
                    session.state.saved.items = vec![
                        json!({"id":"reply","type":"agentMessage","text":"The configured instructions file could not be loaded. ".repeat(8)}),
                        json!({"id":"warning","type":"providerNotice","title":"Configuration warning","text":"The configured instructions file could not be loaded. ".repeat(8),"severity":"warning"}),
                        json!({"id":"compact","type":codex::COMPACTION_ITEM,"status":"completed","trigger":"auto"}),
                    ];
                    cx.notify();
                })));
                draw(cx);
                let response = cx.debug_bounds("codex-message-anchor-0").unwrap();
                let warning = cx.debug_bounds("provider-notice-1").unwrap();
                let body = cx.debug_bounds("provider-notice-text-1-line-0").unwrap();
                assert_eq!(
                    warning.left(),
                    response.left(),
                    "{provider:?}, width {width}"
                );
                assert_eq!(
                    warning.right(),
                    response.right(),
                    "{provider:?}, width {width}"
                );
                assert!(body.left() > warning.left());
                assert!(body.right() <= warning.right());
                // TextView's wrapped descendant can exceed its borderless owner’s nominal
                // synthetic height. Native captures confirm the actual content stays clear
                // of the following event; compare painted content to that event directly.
                let next = cx.debug_bounds("codex-compaction-2").unwrap();
                assert!(
                    body.bottom() <= next.top(),
                    "{provider:?} width {width}: body={body:?} next={next:?}"
                );
                assert!(cx.debug_bounds("codex-error").is_none());
            }
        }
    }

    #[test]
    fn tool_failure_summary_is_local_and_explicit() {
        let command = json!({"type":"commandExecution","command":"npm test","exitCode":1,"status":"completed"});
        assert!(tool_activity_summary(&command, false).starts_with("Command failed"));
        assert!(!is_tool_item(&json!({"type":"providerNotice"})));
        assert!(!tool_failed(
            &json!({"type":"commandExecution","exitCode":0,"status":"completed"})
        ));
    }

    #[gpui::test]
    fn task_strip_docks_above_composer_toggles_and_jumps_to_task_start(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        cx.simulate_resize(gpui::size(gpui::px(1000.0), gpui::px(800.0)));
        draw(cx);
        assert!(cx.debug_bounds("codex-task-strip").is_none());
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Working;
                    session.state.saved.items = vec![
                        json!({"id":"u1", "type":"userMessage", "content":[{"text":"Go"}]}),
                        json!({"id":"a0", "type":"agentMessage", "text":"Long reply.\n\n".repeat(60)}),
                        json!({"id":"grove-plan-t1", "type":"plan", "text":"", "plan":[
                            {"step":"Inspect","status":"completed"},
                            {"step":"Implement","status":"inProgress"},
                            {"step":"Test","status":"pending"}]}),
                        json!({"id":"a1", "type":"agentMessage", "text":"More work.\n\n".repeat(60)}),
                    ];
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(view.read_with(cx, |view, _| view.tasks_expanded));
        assert!(cx.debug_bounds("codex-task-row-2").is_some());
        let initial_header = cx.debug_bounds("codex-task-strip").unwrap();
        cx.simulate_click(initial_header.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(!view.read_with(cx, |view, _| view.tasks_expanded));
        let strip = cx.debug_bounds("codex-task-strip-panel").unwrap();
        let composer = cx.debug_bounds("codex-composer").unwrap();
        let header = cx.debug_bounds("codex-task-strip").unwrap();
        let rem_size = cx.update(|window, _| window.rem_size());
        assert_eq!(header.size.height, rpx(CONTROL_H).to_pixels(rem_size));
        assert_eq!(
            header.top() - strip.top(),
            rpx(SPACE_LG).to_pixels(rem_size) + gpui::px(1.0),
            "the header keeps the tray's top padding inside its border"
        );
        assert!(strip.top() < composer.top());
        assert!(
            strip.left() > composer.left(),
            "tasks share the queue tray inset"
        );
        assert!(
            strip.right() < composer.right(),
            "tasks stay inside the composer rail"
        );
        assert!(cx.debug_bounds("codex-task-row-0").is_none());
        assert!(cx.debug_bounds("codex-task-strip-stale").is_none());
        // Click and keyboard both toggle the in-memory expansion.
        let header = cx.debug_bounds("codex-task-strip").unwrap();
        cx.simulate_click(header.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(view.read_with(cx, |view, _| view.tasks_expanded));
        assert!(cx.debug_bounds("codex-task-row-2").is_some());
        let expanded = cx.debug_bounds("codex-task-strip-panel").unwrap();
        assert!(expanded.top() < strip.top(), "the strip grows upward");
        let expanded_header = cx.debug_bounds("codex-task-strip").unwrap();
        assert_eq!(
            header.top() - strip.top(),
            expanded_header.top() - expanded.top(),
            "collapsed and expanded headers keep the same top corner inset"
        );
        assert_eq!(
            header.left() - strip.left(),
            expanded_header.left() - expanded.left(),
            "collapsed and expanded headers keep the same horizontal corner inset"
        );
        let row = cx.debug_bounds("codex-task-row-1").unwrap();
        let jump = cx.debug_bounds("codex-task-jump-1").unwrap();
        let title = cx.debug_bounds("codex-task-title-1").unwrap();
        assert!(jump.right() <= row.right());
        assert!(
            title.right() <= jump.left(),
            "revealed jump action cannot cover the task title"
        );
        cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::default());
        draw(cx);
        cx.simulate_click(row.center(), gpui::Modifiers::default());
        draw(cx);
        let transcript = cx.debug_bounds("codex-messages").unwrap();
        let anchor = cx.debug_bounds("codex-message-anchor-3").unwrap();
        assert!(
            (anchor.top() - transcript.top()).abs() < gpui::px(1.0),
            "jump aligns the first transcript content after the hidden task update"
        );
        assert!(view.read_with(cx, |view, _| view.composer_collapsed));
        // Answering a clarification keeps the session checklist current.
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.tasks_expanded = false;
                view.session.update(cx, |session, cx| {
                    session.state.saved.items.push(
                        json!({"id":"u2", "type":"userMessage", "content":[{"text":"Both equally"}]}),
                    );
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-task-strip").is_some());
        assert!(cx.debug_bounds("codex-task-strip-stale").is_none());
        let list = view.read_with(cx, |view, _| {
            view.task_cache.as_ref().unwrap().list.clone().unwrap()
        });
        assert_eq!((list.done(), list.total()), (1, 3));
        assert_eq!(list.now_label(), Some("Implement"));
    }

    #[test]
    fn specialized_claude_disclosures_preserve_results_arguments_and_errors() {
        for kind in ["fileChange", "webSearch", "plan"] {
            let item = serde_json::json!({"type":kind, "arguments":{"query":"requested input"}, "result":{"content":[{"text":"returned result"}]}, "error":{"message":"tool failed"}});
            let detail = tool_detail(&item);
            assert!(detail.contains("requested input"));
            assert!(detail.contains("returned result"));
            assert!(detail.contains("tool failed"));
            assert!(!detail.contains("\"content\""));
        }
    }

    #[gpui::test]
    fn unavailable_redirect_keeps_draft_and_enter_queues(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.provider = codex::Provider::Claude;
                    session.state.status = Status::Working;
                    session.state.turn_id = None;
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
                        updates: receive.into(),
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
    fn composer_replaces_send_with_stop_until_turn_finishes(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, _) = fixture(cx);
        draw(cx);
        assert!(cx.debug_bounds("codex-send").is_some());
        assert!(cx.debug_bounds("codex-stop").is_none());
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input
                    .update(cx, |input, cx| input.set_value("Start work", window, cx));
                view.submit(window, cx);
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Working;
                    session.state.turn_id = None;
                    cx.notify();
                });
            });
        });
        assert!(actions
            .try_iter()
            .any(|action| matches!(action, Action::Submit { text, .. } if text == "Start work")));
        draw(cx);
        assert!(cx.debug_bounds("codex-send").is_none());
        assert!(cx.debug_bounds("codex-steer").is_none());
        let stop = cx.debug_bounds("codex-stop").unwrap();
        cx.simulate_click(stop.center(), gpui::Modifiers::default());
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::Interrupt)));
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.turn_id = Some("active-turn".into());
                cx.notify();
            });
        });
        draw(cx);
        let stop = cx.debug_bounds("codex-stop").unwrap();
        cx.simulate_click(stop.center(), gpui::Modifiers::default());
        cx.simulate_click(stop.center(), gpui::Modifiers::default());
        assert_eq!(
            actions
                .try_iter()
                .filter(|action| matches!(action, Action::Interrupt))
                .count(),
            1
        );
        draw(cx);
        assert!(cx.debug_bounds("codex-send").is_none());
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Done;
                session.state.turn_id = None;
                cx.notify();
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-send").is_some());
        assert!(cx.debug_bounds("codex-stop").is_none());
        assert!(cx.debug_bounds("codex-steer").is_none());
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
            let early = super::super::components::shimmer_highlights(label, 0.25);
            let later = super::super::components::shimmer_highlights(label, 0.75);
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
                view.last_content = None; // Model mounting an existing saved transcript.
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
    #[test]
    fn composer_effort_stops_use_only_the_effective_catalog() {
        let mut state = snapshot();
        assert!(CodexView::effort_choices(&state).is_empty());
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            state.saved.provider = provider;
            state.model = Some("alias".into());
            state.resolved_model = Some("resolved".into());
            state.models = vec![
                json!({"model":"alias", "resolvedModel":"resolved", "supportedReasoningEfforts":[{"reasoningEffort":"high"}]}),
            ];
            assert_eq!(CodexView::effort_choices(&state), vec!["high"]);
            state.models[0]["supportedReasoningEfforts"] = json!([
                {"reasoningEffort":"low"}, {"reasoningEffort":"medium"}, {"reasoningEffort":"high"}
            ]);
            assert_eq!(
                CodexView::effort_choices(&state),
                vec!["low", "medium", "high"]
            );
            assert!(!CodexView::effort_choices(&state).contains(&"__configured".into()));
            state.models[0]["supportedReasoningEfforts"] = json!([]);
            assert!(CodexView::effort_choices(&state).is_empty());
        }
        assert_eq!(effort_stop(-1.0, 3), 0);
        assert_eq!(effort_stop(0.5, 3), 1);
        assert_eq!(effort_stop(2.0, 3), 2);
        assert_eq!(effort_key_index("right", 0, 1), None);
        assert_eq!(effort_key_index("end", 0, 3), Some(2));
        assert_eq!(effort_key_index("left", 0, 3), Some(0));
    }

    #[gpui::test]
    fn composer_effort_card_commits_on_release_and_preserves_draft(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.models = vec![json!({"model":"test", "displayName":"Test model", "defaultReasoningEffort":"medium", "supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"medium"},{"reasoningEffort":"high"}]})];
                session.state.model = Some("test".into());
                session.state.effort = Some("medium".into());
                cx.notify();
            });
            view.input.update(cx, |input, cx| input.set_value("Keep my draft", window, cx));
            view.open_menu(ComposerMenu::Settings, window, cx);
        }));
        draw(cx);
        let summary = cx.debug_bounds("codex-model-effort-summary").unwrap();
        let card_model = cx.debug_bounds("codex-card-model-label").unwrap();
        cx.update(|window, _| {
            let label = "Test model · Medium";
            let mut run = window.text_style().to_run(label.len());
            run.font.family = crate::fonts::UI_FAMILY.into();
            let required = truncation_safe_label_width(
                label,
                rpx(TEXT_SMALL).to_pixels(window.rem_size()),
                &run,
                window.text_system(),
            );
            assert!(
                summary.size.width >= required,
                "ordinary model and effort labels must not ellipsize prematurely"
            );
            let model = "Test model";
            let mut run = window.text_style().to_run(model.len());
            run.font.family = crate::fonts::UI_FAMILY.into();
            let required = truncation_safe_label_width(
                model,
                rpx(TEXT_SMALL).to_pixels(window.rem_size()),
                &run,
                window.text_system(),
            );
            assert!(
                card_model.size.width >= required,
                "the card model name must fit before its chevron"
            );
        });
        let trigger = view.read_with(cx, |view, _| view.option_bounds[0].get());
        let card = cx.debug_bounds("codex-options-popup").unwrap();
        assert!(card.bottom() <= trigger.top());
        assert!((trigger.top() - card.bottom() - gpui::px(SPACE_LG)).abs() < gpui::px(1.0));
        let bounds = view.read_with(cx, |view, _| view.effort_bounds.get());
        let end = gpui::point(bounds.right(), bounds.center().y);
        let _ = actions.try_iter().collect::<Vec<_>>();
        cx.simulate_mouse_down(end, gpui::MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::Configure { .. })));
        cx.simulate_mouse_up(end, gpui::MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        assert!(actions.try_iter().any(|action| matches!(action, Action::Configure { effort: Some(effort), model: None, access: None } if effort == "high")));
        cx.simulate_keystrokes("home right");
        draw(cx);
        let values: Vec<_> = actions
            .try_iter()
            .filter_map(|action| match action {
                Action::Configure { effort, .. } => effort,
                _ => None,
            })
            .collect();
        assert_eq!(values, ["low", "medium"]);
        let reset = cx.debug_bounds("codex-model-effort-reset").unwrap();
        cx.simulate_click(reset.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(actions
            .try_iter()
            .any(|action| matches!(action, Action::ResetModelEffort)));
        cx.simulate_keystrokes("escape");
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert!(view.menu.is_none());
            assert_eq!(view.input.read(cx).value().as_ref(), "Keep my draft");
        });
    }

    #[gpui::test]
    fn composer_effort_motion_tracks_pointer_and_settles(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, _) = fixture(cx);
        let advance = |cx: &mut gpui::VisualTestContext, millis| {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(millis));
            draw(cx);
        };
        let aligned = |cx: &mut gpui::VisualTestContext| {
            let thumb = cx.debug_bounds("codex-effort-thumb").unwrap();
            let fill = cx.debug_bounds("codex-effort-fill").unwrap();
            let slider = cx.debug_bounds("codex-effort-slider").unwrap();
            assert_eq!(
                thumb.size.width, thumb.size.height,
                "effort thumb must remain round"
            );
            assert!(
                thumb.left() >= slider.left() && thumb.right() <= slider.right(),
                "the complete effort thumb must stay inside its slider: {thumb:?}, {slider:?}"
            );
            assert!(
                (fill.right() - thumb.right()).abs() < gpui::px(1.0),
                "the rounded fill cap must stay covered by the moving thumb"
            );
            thumb.center().x
        };
        for (provider, supported) in [
            (codex::Provider::Codex, vec!["low", "medium", "high"]),
            (codex::Provider::Claude, vec!["low", "max"]),
        ] {
            cx.update(|window, cx| {
                cx.set_reduce_motion(false);
                view.update(cx, |view, cx| {
                    view.close_menu(cx);
                    view.session.update(cx, |session, cx| {
                        session.state.saved.provider = provider;
                        session.state.model = Some("motion-model".into());
                        session.state.effort = Some("low".into());
                        session.state.models = vec![
                            json!({"model":"motion-model", "supportedReasoningEfforts":supported.iter().map(|effort| json!({"reasoningEffort":effort})).collect::<Vec<_>>()}),
                            json!({"model":"fixed-model", "supportedReasoningEfforts":[{"reasoningEffort":"high"}]}),
                        ];
                        cx.notify();
                    });
                    view.open_menu(ComposerMenu::Settings, window, cx);
                });
            });
            draw(cx);
            view.read_with(cx, |view, cx| {
                assert_eq!(
                    CodexView::effort_choices(&view.session.read(cx).state),
                    supported
                );
            });
            let path = view.read_with(cx, |view, _| view.effort_bounds.get());
            let start = gpui::point(path.left(), path.center().y);
            let end = gpui::point(path.right(), path.center().y);
            let before = aligned(cx);
            let _ = actions.try_iter().collect::<Vec<_>>();
            cx.simulate_click(end, gpui::Modifiers::default());
            draw(cx);
            advance(cx, 16);
            let intermediate = aligned(cx);
            assert!(
                intermediate > before && intermediate < path.right(),
                "click must render motion between supported stops instead of jumping"
            );
            assert_eq!(actions.try_iter().filter(|action| matches!(action, Action::Configure { effort: Some(effort), model: None, access: None } if effort == supported.last().unwrap())).count(), 1);
            // Reverse a moving click: its first frame must start where it was displayed.
            cx.simulate_click(start, gpui::Modifiers::default());
            draw(cx);
            assert!(
                (aligned(cx) - intermediate).abs() < gpui::px(1.0),
                "retargeting must not jump to the old semantic stop"
            );
            advance(cx, 16);
            assert!(aligned(cx) < intermediate && aligned(cx) > path.left());
            advance(cx, MOTION_BASE_MS);
            assert!((aligned(cx) - path.left()).abs() < gpui::px(1.0));
            let _ = actions.try_iter().collect::<Vec<_>>();
            // Move between stops, not onto a dot. Capture must work despite the card's occlusion.
            let between = gpui::point(path.left() + path.size.width * 0.35, path.center().y);
            cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::default());
            cx.simulate_mouse_move(
                between,
                Some(gpui::MouseButton::Left),
                gpui::Modifiers::default(),
            );
            draw(cx);
            advance(cx, 16);
            assert!(
                aligned(cx) > path.left() && aligned(cx) < between.x,
                "dragging must move responsively toward the continuous pointer position"
            );
            advance(cx, MOTION_FAST_MS);
            assert!(
                (aligned(cx) - between.x).abs() < gpui::px(1.0),
                "thumb must follow the pointer between discrete stops"
            );
            assert!(!actions
                .try_iter()
                .any(|action| matches!(action, Action::Configure { .. })));
            cx.simulate_mouse_up(between, gpui::MouseButton::Left, gpui::Modifiers::default());
            draw(cx);
            let index = effort_stop(0.35, supported.len());
            let writes: Vec<_> = actions
                .try_iter()
                .filter_map(|action| match action {
                    Action::Configure { effort, .. } => effort,
                    _ => None,
                })
                .collect();
            assert_eq!(writes, [supported[index]]);
            advance(cx, MOTION_BASE_MS);
            let stop_x = path.left() + path.size.width * effort_fraction(index, supported.len());
            assert!(
                (aligned(cx) - stop_x).abs() < gpui::px(1.0),
                "release must settle onto the committed supported stop"
            );
            assert!(!actions
                .try_iter()
                .any(|action| matches!(action, Action::Configure { .. })));
            // Capture continues outside both the track and the card.
            let outside = gpui::point(
                path.right() + gpui::px(ICON_BTN_W),
                path.top() - gpui::px(ICON_BTN_W),
            );
            cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::default());
            cx.simulate_mouse_move(
                outside,
                Some(gpui::MouseButton::Left),
                gpui::Modifiers::default(),
            );
            draw(cx);
            advance(cx, MOTION_FAST_MS);
            assert!((aligned(cx) - path.right()).abs() < gpui::px(1.0));
            cx.simulate_mouse_up(outside, gpui::MouseButton::Left, gpui::Modifiers::default());
            draw(cx);
            assert_eq!(actions.try_iter().filter(|action| matches!(action, Action::Configure { effort: Some(effort), .. } if effort == supported.last().unwrap())).count(), 1);
            // Enabling Reduce motion midway snaps to the target and stops scheduling frames.
            cx.simulate_click(start, gpui::Modifiers::default());
            draw(cx);
            advance(cx, 16);
            assert!(aligned(cx) > path.left());
            cx.update(|_, cx| cx.set_reduce_motion(true));
            draw(cx);
            assert!((aligned(cx) - path.left()).abs() < gpui::px(1.0));
            cx.update(|window, cx| {
                window.simulate_next_frame(cx);
            });
            draw(cx);
            // Other component callbacks can remain queued. The slider schedules
            // its frames by notifying this view; observe that boundary directly.
            let frame_notifications = Rc::new(Cell::new(0));
            let observed = frame_notifications.clone();
            let frame_watch = cx.update(|_, cx| {
                cx.observe(&view, move |_, _| {
                    observed.set(observed.get() + 1);
                })
            });
            for _ in 0..3 {
                cx.update(|window, cx| {
                    window.simulate_next_frame(cx);
                });
                draw(cx);
            }
            assert_eq!(
                frame_notifications.get(),
                0,
                "reduced slider motion must not request view animation frames"
            );
            drop(frame_watch);
            view.read_with(cx, |view, _| {
                let visual = view.effort_visual.as_ref().unwrap();
                assert!(visual.started.is_none());
                assert_eq!(visual.from, visual.target);
            });
            let _ = actions.try_iter().collect::<Vec<_>>();
            cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::default());
            cx.simulate_mouse_move(
                between,
                Some(gpui::MouseButton::Left),
                gpui::Modifiers::default(),
            );
            draw(cx);
            assert!((aligned(cx) - between.x).abs() < gpui::px(1.0));
            assert!(!actions
                .try_iter()
                .any(|action| matches!(action, Action::Configure { .. })));
            cx.simulate_mouse_up(between, gpui::MouseButton::Left, gpui::Modifiers::default());
            draw(cx);
            assert!((aligned(cx) - stop_x).abs() < gpui::px(1.0));
            assert_eq!(
                actions
                    .try_iter()
                    .filter(|action| matches!(action, Action::Configure { .. }))
                    .count(),
                1
            );
            // Changing selected models must replace the range, including fixed and absent catalogs.
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, cx| {
                        session.state.model = Some("fixed-model".into());
                        session.state.effort = Some("high".into());
                        cx.notify();
                    });
                });
            });
            draw(cx);
            assert!((aligned(cx) - path.center().x).abs() < gpui::px(1.0));
            cx.simulate_click(end, gpui::Modifiers::default());
            draw(cx);
            assert!(!actions
                .try_iter()
                .any(|action| matches!(action, Action::Configure { .. })));
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, cx| {
                        session.state.model = Some("unknown-model".into());
                        cx.notify();
                    });
                });
            });
            draw(cx);
            assert!(cx.debug_bounds("codex-effort-thumb").is_none());
        }
    }

    #[gpui::test]
    fn composer_effort_motion_reconciles_async_reset_and_acknowledged_updates(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.model = Some("same-model".into());
                session.state.effort = Some("high".into());
                session.state.resolved_effort = Some("low".into());
                session.state.models = vec![json!({"model":"same-model", "defaultReasoningEffort":"low", "supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"high"}]})];
                cx.notify();
            });
            view.open_menu(ComposerMenu::Settings, window, cx);
        }));
        draw(cx);
        let path = view.read_with(cx, |view, _| view.effort_bounds.get());
        let assert_stop = |cx: &mut gpui::VisualTestContext, expected| {
            let thumb = cx.debug_bounds("codex-effort-thumb").unwrap();
            let fill = cx.debug_bounds("codex-effort-fill").unwrap();
            assert!(
                (thumb.center().x - expected).abs() < gpui::px(1.0),
                "thumb must match the effective effort after asynchronous updates"
            );
            assert!((fill.right() - thumb.right()).abs() < gpui::px(1.0));
        };
        assert_stop(cx, path.right());
        let _ = actions.try_iter().collect::<Vec<_>>();
        let reset = cx.debug_bounds("codex-model-effort-reset").unwrap();
        cx.simulate_click(reset.center(), gpui::Modifiers::default());
        // send() queues the action; this redraw still sees the pre-reset High snapshot.
        draw(cx);
        assert_stop(cx, path.right());
        assert!(actions
            .try_iter()
            .any(|action| matches!(action, Action::ResetModelEffort)));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.model = None;
                    session.state.resolved_model = Some("same-model".into());
                    session.state.effort = None;
                    session.state.resolved_effort = Some("low".into());
                    cx.notify();
                });
            });
        });
        draw(cx);
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(MOTION_BASE_MS));
        draw(cx);
        assert_stop(cx, path.left());
        view.read_with(cx, |view, cx| {
            let state = &view.session.read(cx).state;
            let choices = CodexView::effort_choices(state);
            assert_eq!(view.effort_index(state, &choices), Some(0));
            assert_eq!(effort_label(&choices[0]), "Low");
        });
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::Configure { .. })));
        // An unsolicited effort change for the same model must also move the thumb.
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.effort = Some("high".into());
                    cx.notify();
                });
            });
        });
        draw(cx);
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(MOTION_BASE_MS));
        draw(cx);
        assert_stop(cx, path.right());
        // Once a local choice is acknowledged, it must not mask later provider updates.
        cx.simulate_click(
            gpui::point(path.left(), path.center().y),
            gpui::Modifiers::default(),
        );
        draw(cx);
        assert_eq!(actions.try_iter().filter(|action| matches!(action, Action::Configure { effort: Some(effort), .. } if effort == "low")).count(), 1);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.effort = Some("low".into());
                    cx.notify();
                });
            });
        });
        draw(cx);
        view.read_with(cx, |view, _| assert!(view.effort_pending.is_none()));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.effort = Some("high".into());
                    cx.notify();
                });
            });
        });
        draw(cx);
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(MOTION_BASE_MS));
        draw(cx);
        assert_stop(cx, path.right());
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::Configure { .. })));
    }

    #[gpui::test]
    fn composer_effort_drag_cancels_on_escape_and_catalog_or_session_change(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|window, cx| view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.model = Some("test".into());
                session.state.models = vec![json!({"model":"test", "supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"high"}]})];
                cx.notify();
            });
            view.open_menu(ComposerMenu::Settings, window, cx);
        }));
        draw(cx);
        let position = view.read_with(cx, |view, _| view.effort_bounds.get().center());
        let _ = actions.try_iter().collect::<Vec<_>>();
        cx.simulate_mouse_down(
            position,
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        draw(cx);
        cx.simulate_keystrokes("escape");
        view.read_with(cx, |view, _| {
            assert!(view.effort_visual.is_none());
            assert!(view.effort_drag.is_none());
        });
        cx.simulate_mouse_up(
            position,
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        draw(cx);
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::Configure { .. })));
        for change_session in [false, true] {
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, cx| {
                        session.state.models[0]["supportedReasoningEfforts"] =
                            json!([{"reasoningEffort":"low"},{"reasoningEffort":"high"}]);
                        cx.notify();
                    });
                    view.open_menu(ComposerMenu::Settings, window, cx);
                });
            });
            draw(cx);
            let position = view.read_with(cx, |view, _| view.effort_bounds.get().center());
            cx.simulate_mouse_down(
                position,
                gpui::MouseButton::Left,
                gpui::Modifiers::default(),
            );
            draw(cx);
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, cx| {
                        if change_session {
                            session.state.saved.key = "different-session".into();
                        } else {
                            session.state.models[0]["supportedReasoningEfforts"] =
                                json!([{ "reasoningEffort":"low" }]);
                        }
                        cx.notify();
                    });
                });
            });
            draw(cx);
            cx.simulate_mouse_up(
                position,
                gpui::MouseButton::Left,
                gpui::Modifiers::default(),
            );
            draw(cx);
            assert!(!actions
                .try_iter()
                .any(|action| matches!(action, Action::Configure { .. })));
        }
    }

    #[gpui::test]
    fn codex_and_claude_access_popup_pointer_selection_dispatches_configuration(
        cx: &mut gpui::TestAppContext,
    ) {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let (view, cx, actions, _) = fixture(cx);
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, cx| {
                        session.state.saved.provider = provider;
                        session.state.permission_modes = ["default", "acceptEdits", "plan"]
                            .map(str::to_owned)
                            .to_vec();
                        cx.notify();
                    });
                });
            });
            draw(cx);
            for rejected in [false, true] {
                if rejected {
                    cx.update(|_, cx| {
                        view.update(cx, |view, cx| {
                            view.session.update(cx, |session, cx| {
                                session.state.error = Some("mode unavailable".into());
                                cx.notify();
                            });
                        });
                    });
                    draw(cx);
                }
                let trigger = view.read_with(cx, |view, _| view.option_bounds[2].get());
                cx.simulate_mouse_move(trigger.center(), None, gpui::Modifiers::default());
                draw(cx);
                cx.executor()
                    .advance_clock(std::time::Duration::from_secs(1));
                draw(cx);
                cx.simulate_click(trigger.center(), gpui::Modifiers::default());
                draw(cx);
                let popup = cx.debug_bounds("codex-options-popup").unwrap();
                // Both fixtures have four rows; use the rendered menu's second row.
                let second_row =
                    gpui::point(popup.center().x, popup.top() + popup.size.height * 0.375);
                cx.simulate_mouse_move(second_row, None, gpui::Modifiers::default());
                draw(cx);
                cx.simulate_mouse_down(
                    second_row,
                    gpui::MouseButton::Left,
                    gpui::Modifiers::default(),
                );
                draw(cx);
                cx.simulate_mouse_up(
                    second_row,
                    gpui::MouseButton::Left,
                    gpui::Modifiers::default(),
                );
                draw(cx);
                let expected = if provider == codex::Provider::Claude {
                    "acceptEdits"
                } else {
                    "workspace-write"
                };
                assert!(actions.try_iter().any(|action| matches!(action, Action::Configure { access: Some(access), model: None, effort: None } if access == expected)));
                assert!(view.read_with(cx, |view, _| view.popup.is_none()));
            }
        }
    }

    #[gpui::test]
    fn claude_restore_defaults_row_and_later_access_selection_dispatch_with_pointer(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.provider = codex::Provider::Claude;
                    session.state.saved.thread_id = None;
                    session.state.permission_modes =
                        ["default", "auto", "acceptEdits", "plan", "dontAsk"]
                            .map(str::to_owned)
                            .to_vec();
                    session.state.access = Some("dontAsk".into());
                    session.state.resolved_access = Some("dontAsk".into());
                    session.state.saved.composer.access = Some("dontAsk".into());
                    cx.notify();
                });
            });
        });
        cx.simulate_resize(gpui::size(gpui::px(1800.), gpui::px(1430.)));
        draw(cx);
        let trigger = view.read_with(cx, |view, _| view.option_bounds[2].get());
        cx.simulate_click(trigger.center(), gpui::Modifiers::default());
        draw(cx);
        let popup = cx.debug_bounds("codex-options-popup").unwrap();
        let reset = gpui::point(popup.left() + gpui::px(45.), popup.bottom() - gpui::px(17.));
        cx.simulate_mouse_move(reset, None, gpui::Modifiers::default());
        draw(cx);
        cx.simulate_click(reset, gpui::Modifiers::default());
        draw(cx);
        assert!(actions
            .try_iter()
            .any(|action| matches!(action, Action::ResetConfiguration)));
        assert!(view.read_with(cx, |view, _| view.popup.is_none()));
        cx.simulate_click(trigger.center(), gpui::Modifiers::default());
        draw(cx);
        let popup = cx.debug_bounds("codex-options-popup").unwrap();
        let first = gpui::point(popup.left() + gpui::px(45.), popup.top() + gpui::px(17.));
        cx.simulate_mouse_move(first, None, gpui::Modifiers::default());
        draw(cx);
        cx.simulate_click(first, gpui::Modifiers::default());
        draw(cx);
        assert!(actions.try_iter().any(|action| matches!(action, Action::Configure { access: Some(access), .. } if access == "default")));
        assert!(view.read_with(cx, |view, _| view.popup.is_none()));
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
        // Identical answers from a previous question are not this submission's echo.
        state
            .saved
            .items
            .push(json!({"type":"userMessage", "id":"old-answer",
            "content":[{"type":"text", "text":"Choose scope\nOnly focus and scrolling"}]}));
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
                    // Matching user text while the form is open is not its answer echo.
                    session.state.saved.items.push(json!({"type":"userMessage", "id":"before-submit",
                        "content":[{"type":"text", "text":"Choose scope\nOnly focus and scrolling"}]}));
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
        let epoch = view.read_with(cx, |view, cx| view.session.read(cx).state.interaction_epoch);
        let answer = actions_at_epoch(&actions, epoch)
            .into_iter()
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
                    let thread_id = session.state.saved.thread_id.clone();
                    session
                        .state
                        .question_answer_acks
                        .push(codex::QuestionAnswerAck {
                            sequence: 1,
                            id: json!("async-ui"),
                            thread_id,
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
        // A later user-message echo replaces the receipt, preserving one record.
        // An unrelated message must not discard the acknowledged answers.
        for (text, receipt_visible) in [
            ("Unrelated message", true),
            ("Choose scope\nOnly focus and scrolling", false),
        ] {
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, cx| {
                        let index = session.state.saved.items.len();
                        session.state.saved.items.push(json!({
                            "type":"userMessage", "id":format!("echo-{index}"),
                            "content":[{"type":"text", "text":text}]
                        }));
                        cx.notify();
                    });
                });
            });
            draw(cx);
            assert_eq!(
                cx.debug_bounds("question-receipt-0").is_some(),
                receipt_visible
            );
        }
        view.read_with(cx, |view, cx| {
            assert!(view.question_receipts.is_empty());
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
        let epoch = view.read_with(cx, |view, cx| view.session.read(cx).state.interaction_epoch);
        assert!(actions_at_epoch(&actions, epoch)
            .into_iter()
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
        assert!(actions_at_epoch(&actions, epoch)
            .into_iter()
            .any(|action| matches!(action, Action::AnswerQuestions(_, Some(answers)) if answers["count"] == vec!["12"] && answers["title"] == vec!["Quarterly briefing"])));
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
        assert!(actions_at_epoch(&actions, epoch)
            .into_iter()
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
                    let thread_id = session.state.saved.thread_id.clone();
                    let turn_id = session.state.turn_id.clone();
                    session
                        .state
                        .question_answer_acks
                        .push(codex::QuestionAnswerAck {
                            sequence: 1,
                            id: request.id.clone(),
                            thread_id,
                            turn_id,
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
        assert!(view.read_with(cx, |view, _| {
            view.question_drafts[&key].questions[0]
                .selected
                .contains(&1)
        }));
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
        let epoch = view.read_with(cx, |view, cx| view.session.read(cx).state.interaction_epoch);
        let sent: Vec<_> = actions_at_epoch(&actions, epoch)
            .into_iter()
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
    fn pending_approval_buttons_are_visible_and_clicks_emit_one_typed_response(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        for width in [1024.0, 600.0] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(480.0)));
            for (request_id, selector, expected_choice) in [
                ("approve", "approval-0-0", "allow_once"),
                ("decline", "approval-0-2", "deny"),
            ] {
                cx.update(|_, cx| {
                    view.update(cx, |view, cx| {
                        view.session.update(cx, |session, cx| {
                            session.state.status = Status::Waiting;
                            session.state.approvals = vec![json!({
                                "id": request_id,
                                "method": "item/commandExecution/requestApproval",
                                "params": {"command": "pwd", "cwd": "/tmp"}
                            })];
                            cx.notify();
                        });
                    });
                });
                draw(cx);
                let epoch =
                    view.read_with(cx, |view, cx| view.session.read(cx).state.interaction_epoch);
                let controls = cx.debug_bounds("agent-controls").unwrap();
                let button = cx
                    .debug_bounds(selector)
                    .expect("pending approval exposes an actionable button");
                assert!(button.size.width > gpui::px(0.0));
                assert!(button.size.height > gpui::px(0.0));
                let control_height =
                    cx.update(|window, _| rpx(CONTROL_H).to_pixels(window.rem_size()));
                assert!(
                    button.size.height <= control_height,
                    "approval uses compact transcript controls: {button:?}"
                );
                assert!(
                    controls.contains(&button.origin),
                    "approval button {selector}: {button:?}; controls: {controls:?}"
                );
                let composer = cx.debug_bounds("codex-composer").unwrap();
                assert!(controls.bottom() <= composer.top());
                assert!(controls.left() >= composer.left());
                assert!(controls.right() <= composer.right());
                assert!(cx.debug_bounds("codex-live-activity").is_none());
                assert!(button.right() <= controls.right());
                assert!(button.bottom() <= controls.bottom());
                assert!(button.bottom() <= gpui::px(480.0));
                cx.simulate_click(button.center(), gpui::Modifiers::default());
                draw(cx);
                cx.simulate_click(button.center(), gpui::Modifiers::default());
                draw(cx);
                let responses: Vec<_> = actions_at_epoch(&actions, epoch)
                    .into_iter()
                    .filter_map(|action| match action {
                        Action::RespondApproval {
                            id,
                            choice,
                            input,
                            feedback,
                        } => Some((id, choice, input, feedback)),
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    responses,
                    vec![(json!(request_id), expected_choice.into(), None, None)]
                );
            }
        }
    }

    #[gpui::test]
    fn matching_prefix_requires_disclosure_before_sending(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, _) = fixture(cx);
        cx.simulate_resize(gpui::size(gpui::px(600.0), gpui::px(700.0)));
        let approval = json!({"id":"prefix", "method":"item/commandExecution/requestApproval", "params":{
            "command":"cargo test --manifest-path vendor/gpui-component/ui/Cargo.toml --lib text::window_selection::tests --no-run --locked",
            "cwd":"/tmp/project", "proposedExecpolicyAmendment":["cargo","test"]
        }});
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.status = Status::Waiting;
                    session.state.approvals = vec![approval.clone()];
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("approval-0-2").is_none());
        let scroll_controls_to_bottom = |cx: &mut gpui::VisualTestContext| {
            let controls = cx.debug_bounds("agent-controls").unwrap();
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: controls.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(-100000.0))),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            });
            draw(cx);
        };
        scroll_controls_to_bottom(cx);
        let disclosure = cx.debug_bounds("approval-policy-0").unwrap();
        cx.simulate_click(disclosure.center(), gpui::Modifiers::default());
        draw(cx);
        scroll_controls_to_bottom(cx);
        let button = cx.debug_bounds("approval-0-2").unwrap();
        let controls = cx.debug_bounds("agent-controls").unwrap();
        assert!(button.top() >= controls.top() && button.bottom() <= controls.bottom());
        cx.simulate_click(button.center(), gpui::Modifiers::default());
        draw(cx);
        let epoch = view.read_with(cx, |view, cx| view.session.read(cx).state.interaction_epoch);
        let responses: Vec<_> = actions_at_epoch(&actions, epoch)
            .into_iter()
            .filter_map(|action| match action {
                Action::RespondApproval { choice, .. } => Some(choice),
                _ => None,
            })
            .collect();
        assert_eq!(responses, vec!["execpolicy"]);
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
        let key = view.read_with(cx, |view, cx| {
            CodexView::elicitation_key(&request, view.session.read(cx).state.interaction_epoch)
        });
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
                    &CodexView::approval_key(
                        &approval,
                        view.session.read(cx).state.interaction_epoch,
                    ),
                    json!("approval"),
                    "allow_once".into(),
                    cx,
                );
            });
        });
        let epoch = view.read_with(cx, |view, cx| view.session.read(cx).state.interaction_epoch);
        let sent = actions_at_epoch(&actions, epoch);
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
        let key = view.read_with(cx, |view, cx| {
            CodexView::approval_key(&approval, view.session.read(cx).state.interaction_epoch)
        });
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
                    &CodexView::elicitation_key(
                        &request,
                        view.session.read(cx).state.interaction_epoch,
                    ),
                    json!("browser"),
                    "decline",
                    cx,
                );
            });
        });
        let epoch = view.read_with(cx, |view, cx| view.session.read(cx).state.interaction_epoch);
        let sent = actions_at_epoch(&actions, epoch);
        assert!(sent.iter().any(|action| matches!(action, Action::RespondApproval { input: Some(input), .. } if input == &json!({"command":"ls"}))));
        assert!(sent.iter().any(|action| matches!(action, Action::RespondElicitation { action, content: None, .. } if action == "decline")));
    }

    #[gpui::test]
    fn child_approval_draft_survives_parent_turn_and_old_control_expires_on_reconnect(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        let approval = json!({"id":"reused","method":"item/commandExecution/requestApproval","claudeInput":{"command":"pwd"},"groveAgent":"Reviewer"});
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.turn_id = Some("parent-turn".into());
                    session.state.status = Status::Waiting;
                    session.state.approvals.push(approval.clone());
                    session.state.interaction_owners.insert(
                        "\"reused\"".into(),
                        grove_core::interaction::InteractionOwner {
                            thread: "child".into(),
                            turn: Some("child-turn".into()),
                            agent: Some("Reviewer · child".into()),
                        },
                    );
                    cx.notify();
                });
            });
        });
        draw(cx);
        let old_epoch =
            view.read_with(cx, |view, cx| view.session.read(cx).state.interaction_epoch);
        let old_key = CodexView::approval_key(&approval, old_epoch);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let input = view.approval_drafts[&old_key].input.clone();
                view.toggle_approval_edit(&old_key, cx);
                input.update(cx, |input, cx| {
                    input.set_value("{\"command\":\"cargo test\"}", window, cx);
                });
                view.session.update(cx, |session, cx| {
                    session.state.turn_id = None;
                    session.state.status = Status::Waiting;
                    cx.notify();
                });
            });
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.approval_drafts[&old_key].input.read(cx).value(),
                "{\"command\":\"cargo test\"}"
            );
        });
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.interaction_epoch += 1;
                    cx.notify();
                });
            });
        });
        draw(cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.respond_approval(&old_key, json!("reused"), "allow_once".into(), cx);
            });
        });
        assert!(
            actions.try_iter().next().is_none(),
            "a stale approval control must not act on a reused request id"
        );
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
        let key = view.read_with(cx, |view, cx| {
            CodexView::elicitation_key(&request, view.session.read(cx).state.interaction_epoch)
        });
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
    fn work_headers_show_seconds_until_two_minutes_and_skip_unknown_duration() {
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
            subagents: Vec::new(),
        });
        for (duration_ms, expected) in [
            (0, "Worked for 0s"),
            (23_900, "Worked for 23s"),
            (45_000, "Worked for 45s"),
            (59_999, "Worked for 59s"),
            (60_000, "Worked for 1m 0s"),
            (119_999, "Worked for 1m 59s"),
            (120_000, "Worked for 2m"),
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
        assert_eq!(work_header_at(&state, 1).unwrap().1, "Working for 0s");
        state.work_started = Some(
            std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(125))
                .unwrap(),
        );
        assert_eq!(work_header_at(&state, 1).unwrap().1, "Working for 2m");
    }

    #[gpui::test]
    fn expanded_work_reports_empty_parent_activity_for_both_providers(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, updates) = fixture(cx);
        view.update(cx, |view, _| {
            view.expanded.insert("work-0".into());
        });
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let mut state = snapshot();
            state.saved.provider = provider;
            state.status = Status::Working;
            state.saved.items = vec![
                json!({"id":"prompt","type":"userMessage","content":[{"text":"Work"}]}),
                json!({"id":"child","type":"subAgentActivity","kind":"started","agentThreadId":"child-1"}),
            ];
            assert!(!work_has_parent_activity(&state, 1));
            updates.send(state.clone()).unwrap();
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(40));
            draw(cx);
            assert!(cx.debug_bounds("codex-work-empty-1").is_some());
            assert!(cx.debug_bounds("codex-tool-group-1").is_none());
            state.saved.items.push(json!({"id":"first","type":"commandExecution","command":"pwd","status":"completed"}));
            state.saved.items.push(
                json!({"id":"last","type":"commandExecution","command":"ls","status":"inProgress"}),
            );
            assert!(work_has_parent_activity(&state, 1));
            updates.send(state).unwrap();
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(40));
            draw(cx);
            assert!(cx.debug_bounds("codex-work-empty-1").is_none());
            assert!(cx.debug_bounds("codex-tool-row-label-2").is_some());
            assert!(cx.debug_bounds("codex-tool-row-label-3").is_some());
        }
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
            "Running git status --short…"
        );
        assert_eq!(
            tool_activity_summary(&item, false),
            "Ran git status --short"
        );
    }

    #[test]
    fn active_tool_rows_describe_work_while_preserving_raw_disclosures() {
        use serde_json::json;
        for (tool, expected) in [
            ("sleep", "Sleeping…"),
            ("clock__sleep", "Sleeping…"),
            ("wait", "Waiting…"),
            ("functions__wait", "Waiting…"),
            ("wait_agent", "Waiting…"),
            ("thought", "Thinking…"),
            ("Read", "Reading…"),
            ("Write", "Writing…"),
            ("Edit", "Editing…"),
            ("MultiEdit", "Editing…"),
            ("Grep", "Searching…"),
            ("Glob", "Searching…"),
            ("WebSearch", "Searching…"),
            ("WebFetch", "Fetching…"),
            ("custom_operation", "Using custom_operation…"),
        ] {
            let item = json!({"type":"mcpToolCall","server":"fixture","tool":tool,"arguments":{"secret":"input"}});
            assert_eq!(tool_activity_summary(&item, true), expected);
            let raw = format!("Tool · fixture / {tool}");
            assert_eq!(tool_summary(&item), raw);
            assert_eq!(
                tool_activity_summary(&item, false),
                if expected == "Waiting…" {
                    expected
                } else {
                    raw.as_str()
                }
            );
            assert!(tool_detail(&item).contains("input"));
        }
        let reasoning = json!({"type":"reasoning"});
        assert_eq!(tool_activity_summary(&reasoning, true), "Thinking…");
        assert_eq!(tool_activity_summary(&reasoning, false), "Thinking…");
        assert_eq!(
            tool_activity_summary(&json!({"type":"fileChange"}), true),
            "Updating files…"
        );
        assert_eq!(
            tool_activity_summary(&json!({"type":"webSearch"}), true),
            "Searching the web…"
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
        assert!(cx.debug_bounds("codex-message-row-0").is_none());
        cx.update(|window, cx| view.update(cx, |view, cx| view.submit(window, cx)));
        draw(cx);
        assert!(cx.debug_bounds("codex-message-row-0").is_some());
        assert!(cx.debug_bounds("codex-message-row-1").is_none());
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
        assert!(cx.debug_bounds("codex-message-row-0").is_some());
        let mut accepted = snapshot();
        accepted.status = Status::Working;
        accepted.accepted_submission = view.read_with(cx, |view, _| view.submitted_ack);
        accepted.turn_id = Some("turn-ui".into());
        updates.send(accepted).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(view.read_with(cx, |view, cx| view.input.read(cx).value().is_empty()));
        assert!(cx.debug_bounds("codex-message-row-0").is_some());
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
            (
                "/tmp/Option%20C/main.rs#L42",
                "file:///tmp/Option%20C/main.rs#L42",
            ),
            ("https://example.com/a?q=b#c", "https://example.com/a?q=b#c"),
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
    fn codex_inline_html_has_no_document_toolbar(cx: &mut gpui::TestAppContext) {
        inline_html_toolbar_fixture(cx, codex::Provider::Codex);
    }

    #[gpui::test]
    fn claude_inline_html_has_no_document_toolbar(cx: &mut gpui::TestAppContext) {
        inline_html_toolbar_fixture(cx, codex::Provider::Claude);
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

    #[gpui::test]
    fn explicit_browser_html_link_omits_inline_preview_action(cx: &mut gpui::TestAppContext) {
        let (view, cx, _, updates) = fixture(cx);
        let mut response = snapshot();
        response.status = codex::Status::Done;
        response.saved.items.push(serde_json::json!({
            "type":"agentMessage", "id":"browser-document",
            "text":"[Open in browser](large.html)"
        }));
        updates.send(response).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(cx.debug_bounds("html-browser-link-only").is_some());
        assert!(cx.debug_bounds("html-preview-link").is_none());
        assert!(cx.debug_bounds("html-artifact").is_none());
        assert_eq!(
            view.read_with(cx, |view, cx| view.html.read(cx).opened_count()),
            0
        );
        assert!(cx.opened_url().is_none());
    }

    #[gpui::test]
    fn native_html_link_click_reveals_preview_without_external_open(cx: &mut gpui::TestAppContext) {
        let (view, cx, _, updates) = fixture(cx);
        let checkout = tempfile::tempdir().unwrap();
        let cwd = checkout.path().to_string_lossy().into_owned();
        fs_err::write(
            checkout.path().join("launch.html"),
            "<!doctype html><html><body>Launch</body></html>",
        )
        .unwrap();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.set_checkout_context("Main checkout".into(), "main".into(), cx);
                view.session.update(cx, |session, cx| {
                    session.state.saved.cwd.clone_from(&cwd);
                    cx.notify();
                });
            });
        });
        let mut response = snapshot();
        response.saved.cwd = cwd;
        response.saved.items.push(serde_json::json!({"type":"agentMessage", "id":"answer", "text":"[Launch checklist](launch.html)"}));
        updates.send(response).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, cx| view.html.read(cx).opened_count()),
            0
        );
        assert!(
            cx.debug_bounds("html-open-browser").is_some(),
            "relative HTML links must render the independent browser control even with a display checkout label"
        );
        let body = cx.debug_bounds("codex-message-body-0").unwrap();
        cx.simulate_click(
            body.origin + gpui::point(gpui::px(8.), gpui::px(8.)),
            gpui::Modifiers::default(),
        );
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, cx| view.html.read(cx).opened_count()),
            1
        );
        assert!(
            cx.debug_bounds("html-artifact").is_some(),
            "HtmlSession notifications must redraw the parent transcript immediately"
        );
        assert!(
            cx.opened_url().is_none(),
            "primary activation must not send HTML to the platform opener"
        );
    }

    #[gpui::test]
    fn native_html_completed_output_uses_real_cwd_with_display_checkout_label(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, updates) = fixture(cx);
        let checkout = tempfile::tempdir().unwrap();
        let cwd = checkout.path().to_string_lossy().into_owned();
        fs_err::write(
            checkout.path().join("launch.html"),
            "<!doctype html><html><body>Completed launch output</body></html>",
        )
        .unwrap();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.set_checkout_context("Main checkout".into(), "main".into(), cx);
                view.session.update(cx, |session, cx| {
                    session.state.saved.cwd.clone_from(&cwd);
                    cx.notify();
                });
            });
        });
        draw(cx);
        let mut completed = snapshot();
        completed.saved.cwd = cwd;
        completed.status = Status::Done;
        completed.saved.items.push(serde_json::json!({"type":"agentMessage", "id":"completed-html", "text":"The completed output is [Launch checklist](launch.html)."}));
        updates.send(completed).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(cx.debug_bounds("html-open-browser").is_some());
        assert_eq!(
            view.read_with(cx, |view, cx| view.html.read(cx).opened_count()),
            1,
            "a completed relative HTML output must auto-open using saved.cwd without a rendered busy snapshot"
        );
        assert!(
            cx.debug_bounds("html-artifact").is_some(),
            "completed output must actually appear inline"
        );
        let artifact_ids = view.read_with(cx, |view, cx| view.html.read(cx).artifact_ids());
        cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, cx| view.html.read(cx).artifact_ids()),
            artifact_ids
        );
        assert!(cx.opened_url().is_none());
    }

    #[gpui::test]
    fn native_html_multiple_fences_preserve_artifact_identity_during_streaming(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, updates) = fixture(cx);
        let mut response = snapshot();
        response.saved.items.push(serde_json::json!({"type":"agentMessage", "id":"answer", "text":"```html\n<html><body>First</body></html>\n```\n\n~~~html\n<html><body>Second</body></html>\n~~~\n\n```html\n<html>Unfinished"}));
        updates.send(response.clone()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, cx| view.html.read(cx).artifact_count()),
            2
        );
        let ids = view.read_with(cx, |view, cx| {
            assert!(view
                .html
                .read(cx)
                .artifact_owners_match(view.html.entity_id(), cx));
            view.html.read(cx).artifact_ids()
        });
        response.saved.items[0]["text"] = serde_json::Value::String(format!(
            "{} appended stream",
            response.saved.items[0]["text"].as_str().unwrap()
        ));
        updates.send(response).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, cx| view.html.read(cx).artifact_count()),
            2,
            "streaming outside complete fences must not mount replacement artifacts"
        );
        assert_eq!(
            view.read_with(cx, |view, cx| view.html.read(cx).artifact_ids()),
            ids
        );
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
        response.saved.items[0]["text"] = serde_json::json!(
            "quick selection value\n\n## Details\n\n- formatted list\n\n```rust\n    let selected = true;\n```\n\nMore live text."
        );
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
    fn composer_markdown_newline_keeps_source_undo_and_send_for_both_providers(
        cx: &mut gpui::TestAppContext,
    ) {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let (view, cx, actions, _updates) = fixture(cx);
            let source = "> 9. [x] é🦀";
            let continued = "> 9. [x] é🦀\n> 10. [ ] ";
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.session
                        .update(cx, |session, _| session.state.saved.provider = provider);
                    view.input.update(cx, |input, cx| {
                        input.set_value(source, window, cx);
                        input.set_selected_range(source.len()..source.len(), cx);
                        input.focus(window, cx);
                    });
                });
            });
            draw(cx);
            cx.simulate_keystrokes("shift-enter");
            draw(cx);
            view.read_with(cx, |view, cx| {
                assert_eq!(view.input.read(cx).value().as_ref(), continued);
            });
            // The fixture exposes the provider command channel without running
            // its reducer; verify the actual draft command at that boundary.
            let emitted: Vec<_> = actions.try_iter().collect();
            assert!(emitted
                .iter()
                .any(|action| matches!(action, Action::Draft(text) if text == continued)));
            assert!(!emitted
                .iter()
                .any(|action| matches!(action, Action::Submit { .. })));
            cx.simulate_keystrokes(if cfg!(target_os = "macos") {
                "cmd-z"
            } else {
                "ctrl-z"
            });
            draw(cx);
            view.read_with(cx, |view, cx| {
                assert_eq!(view.input.read(cx).value().as_ref(), source);
            });
            cx.simulate_keystrokes(if cfg!(target_os = "macos") {
                "cmd-shift-z"
            } else {
                "ctrl-y"
            });
            draw(cx);
            view.read_with(cx, |view, cx| {
                assert_eq!(view.input.read(cx).value().as_ref(), continued);
            });
            cx.simulate_keystrokes("enter");
            draw(cx);
            let sent: Vec<_> = actions.try_iter().collect();
            assert!(
                sent.iter().any(
                    |action| matches!(action, Action::Submit { text, .. } if text == continued)
                ),
                "provider={provider:?}; actions={:?}; state={:?}; error={:?}",
                sent.iter()
                    .map(|action| match action {
                        Action::Submit { text, .. } => format!("Submit({text:?})"),
                        Action::Draft(text) => format!("Draft({text:?})"),
                        _ => format!("{:?}", std::mem::discriminant(action)),
                    })
                    .collect::<Vec<_>>(),
                view.read_with(cx, |view, cx| view.session.read(cx).state.status),
                view.read_with(cx, |view, cx| view.session.read(cx).state.error.clone())
            );
        }
    }

    #[gpui::test]
    fn composer_markdown_newline_preserves_literal_paste_and_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        for (source, range, expected) in [
            ("## Heading", 10..10, "## Heading\n"),
            ("```\n  - literal", 15..15, "```\n  - literal\n  "),
            ("- é🦀 tail", 8..8, "- é🦀\n-  tail"),
            ("- é🦀 tail", 2..8, "- \n tail"),
            ("> - [ ] ", 8..8, "> "),
        ] {
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.input.update(cx, |input, cx| {
                        input.set_value(source, window, cx);
                        input.set_selected_range(range.clone(), cx);
                        input.focus(window, cx);
                    });
                });
            });
            draw(cx);
            cx.simulate_keystrokes("shift-enter");
            draw(cx);
            view.read_with(cx, |view, cx| {
                assert_eq!(view.input.read(cx).value().as_ref(), expected, "{source:?}");
            });
        }
        let pasted = "- first\n- second\n";
        cx.update(|window, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(pasted.into()));
            view.update(cx, |view, cx| {
                view.input
                    .update(cx, |input, cx| input.set_value("", window, cx));
            });
        });
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-v"
        } else {
            "ctrl-v"
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.input.read(cx).value().as_ref(), pasted);
        });
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
    fn codex_selection_reply_pointer_capture_preserves_draft_and_sends_canonical_quote(
        cx: &mut gpui::TestAppContext,
    ) {
        selection_reply_pointer_fixture(cx, codex::Provider::Codex);
    }
    #[gpui::test]
    fn claude_selection_reply_pointer_capture_preserves_draft_and_sends_canonical_quote(
        cx: &mut gpui::TestAppContext,
    ) {
        selection_reply_pointer_fixture(cx, codex::Provider::Claude);
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

    #[test]
    fn selection_reference_projection_preserves_literal_quote_and_rejects_partial_envelopes() {
        let quote = codex::ReferenceDraft {
            id: "reference".into(),
            thread_id: Some("thread".into()),
            sources: vec![codex::ReferenceSource {
                message_id: "source".into(),
                role: "Assistant".into(),
            }],
            text: "  /clear\n```rust\n<literal>\n\nUser reply:\n```".into(),
        };
        let canonical = quote.serialize("my reply");
        let (restored, reply) = reference_prompt(&canonical).unwrap();
        assert_eq!(restored, quote);
        assert_eq!(reply, "my reply");
        assert!(reference_prompt_copy(&canonical).contains(&quote.text));
        assert!(reference_prompt_markdown(&canonical).contains(&quote.text));
        assert!(!reference_prompt_copy(&canonical).contains("message_id"));
        let partial = "Referenced transcript selection (quoted data):\n{}\n\nUser reply:\nreply";
        assert_eq!(reference_prompt_copy(partial), partial);
    }

    #[gpui::test]
    fn selection_reference_preview_is_bounded_at_narrow_width_and_zoom(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, updates) = fixture_with_navigation(cx, true);
        let mut response = snapshot();
        response.saved.items.push(json!({"id":"source", "type":"agentMessage", "text":"long literal selection ".repeat(200)}));
        updates.send(response).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.set_reference(
                    Some(codex::ReferenceDraft {
                        id: "long".into(),
                        thread_id: Some("thread-ui".into()),
                        sources: vec![codex::ReferenceSource {
                            message_id: "source".into(),
                            role: "Assistant".into(),
                        }],
                        text: "long literal selection ".repeat(200),
                    }),
                    cx,
                );
                view.input.focus_handle(cx).focus(window, cx);
            });
        });
        for (width, rem) in [(1000., 16.), (400., 16.), (400., 20.)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(900.)));
            cx.update(|window, _| window.set_rem_size(gpui::px(rem)));
            for _ in 0..4 {
                cx.update(gpui::Window::simulate_next_frame);
                draw(cx);
            }
            let text = cx.debug_bounds("codex-reference-text").unwrap();
            let preview = cx.debug_bounds("codex-reference-preview").unwrap();
            let composer = cx.debug_bounds("codex-composer").unwrap();
            assert!(
                composer.bottom() <= gpui::px(900.),
                "composer must fit within the pane: {composer:?}"
            );
            assert!(text.size.height <= gpui::px(60. * rem / 16. + 1.));
            assert!(preview.left() >= gpui::px(0.) && preview.right() <= gpui::px(width));
            assert!((preview.left() - composer.left()).abs() < gpui::px(1.));
            assert!(
                preview.bottom() >= composer.top(),
                "preview attaches to composer"
            );
            let disclosure = cx.debug_bounds("codex-reference-disclosure").unwrap();
            cx.simulate_click(disclosure.center(), gpui::Modifiers::default());
            draw(cx);
            view.read_with(cx, |view, _| assert!(view.reference_expanded));
            let text = cx.debug_bounds("codex-reference-text").unwrap();
            assert!(text.size.height <= gpui::px(200. * rem / 16. + 1.));
            let mut focused = false;
            for _ in 0..20 {
                cx.simulate_keystrokes("tab");
                draw(cx);
                focused = cx.update(|window, cx| {
                    view.read(cx).reference_disclosure_focus.is_focused(window)
                });
                if focused {
                    break;
                }
            }
            assert!(focused, "Quote disclosure must be reachable by Tab");
            cx.simulate_keystrokes("space");
            draw(cx);
            view.read_with(cx, |view, _| assert!(!view.reference_expanded));
        }
    }

    #[gpui::test]
    fn selection_reply_source_disappearance_clears_native_selected_text(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_, cx, _, updates) = fixture(cx);
        let mut response = snapshot();
        response.saved.items.push(serde_json::json!({"type":"agentMessage", "id":"removed-source", "text":"selected text disappears"}));
        updates.send(response).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
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
        cx.simulate_mouse_up(end, gpui::MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        assert!(!cx
            .update(gpui_component::WindowExt::selected_text)
            .is_empty());
        updates.send(snapshot()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(cx.debug_bounds("codex-selection-reply").is_none());
        assert!(cx
            .update(gpui_component::WindowExt::selected_text)
            .is_empty());
    }

    #[gpui::test]
    fn selection_reference_queue_editor_saves_its_quote_and_restores_main_reference(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, actions, _) = fixture(cx);
        let quote = |id: &str| codex::ReferenceDraft {
            id: id.into(),
            thread_id: Some("thread-ui".into()),
            sources: vec![codex::ReferenceSource {
                message_id: "source".into(),
                role: "Assistant".into(),
            }],
            text: format!("literal {id} quote"),
        };
        let main = quote("main");
        let queued = quote("queued");
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input
                    .update(cx, |input, cx| input.set_value("main draft", window, cx));
                view.set_reference(Some(main.clone()), cx);
                view.session.update(cx, |session, _| {
                    session.state.queued_prompts.push(codex::QueuedPrompt {
                        id: 81,
                        text: queued.serialize("queued reply"),
                        images: Vec::new(),
                    });
                });
                view.begin_queue_edit(81, cx);
                view.session
                    .update(cx, |session, _| session.state.queue_edit = Some(81));
            });
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.input.read(cx).value().as_ref(), "queued reply");
            assert_eq!(
                view.queue_editor.as_ref().unwrap().reference.as_ref(),
                Some(&queued)
            );
            assert_eq!(view.reference.as_ref(), Some(&main));
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input
                    .update(cx, |input, cx| input.set_value("edited reply", window, cx));
                view.save_queue_edit(cx);
            });
        });
        assert!(actions.try_iter().any(|action| matches!(action, Action::SaveQueuedEdit { id:81, text, .. } if text == queued.serialize("edited reply"))));
        cx.update(|window, cx| view.update(cx, |view, cx| view.restore_queue_draft(window, cx)));
        view.read_with(cx, |view, cx| {
            assert_eq!(view.input.read(cx).value().as_ref(), "main draft");
            assert_eq!(view.reference.as_ref(), Some(&main));
        });
    }

    #[gpui::test]
    fn selection_reference_requires_current_native_message_owners_and_preserves_order(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, updates) = fixture(cx);
        let mut response = snapshot();
        response.saved.items = vec![
            serde_json::json!({"type":"userMessage", "id":"user-source", "content":[{"type":"text","text":"user passage"}]}),
            serde_json::json!({"type":"agentMessage", "id":"assistant-source", "text":"assistant passage"}),
        ];
        updates.send(response).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                let state = view.session.read(cx).state.shared();
                let owner = |id: &str| view.selection_views[&format!("thread-ui:{id}")].entity_id();
                let mut selection = TextSelectionSnapshot {
                    text: "user passage\nassistant passage".into(),
                    spans: vec![
                        gpui_component::text::TextSelectionSpan {
                            owner: owner("user-source"),
                            text: "user passage".into(),
                        },
                        gpui_component::text::TextSelectionSpan {
                            owner: owner("assistant-source"),
                            text: "assistant passage".into(),
                        },
                    ],
                    anchor: gpui::point(gpui::px(0.), gpui::px(0.)),
                    endpoint: gpui::point(gpui::px(1.), gpui::px(1.)),
                    is_selecting: false,
                };
                let quote = view.selection_reference(&selection, &state).unwrap();
                assert_eq!(
                    quote
                        .sources
                        .iter()
                        .map(|s| s.message_id.as_str())
                        .collect::<Vec<_>>(),
                    ["user-source", "assistant-source"]
                );
                selection.spans[1].owner = view.input.entity_id();
                assert!(
                    view.selection_reference(&selection, &state).is_none(),
                    "composer/tool/HTML owners cannot qualify"
                );
                selection.spans.pop();
                let mut switched = (*state).clone();
                switched.saved.thread_id = Some("another-thread".into());
                assert!(
                    view.selection_reference(&selection, &switched).is_none(),
                    "old thread owners cannot qualify"
                );
                let mut removed = (*state).clone();
                removed.saved.items.clear();
                assert!(
                    view.selection_reference(&selection, &removed).is_none(),
                    "removed sources cannot qualify"
                );
            });
        });
    }

    #[gpui::test]
    fn selection_reference_replacement_remove_and_late_accept_preserve_newer_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, updates) = fixture_with_navigation(cx, true);
        let quote = |id: &str| codex::ReferenceDraft {
            id: id.into(),
            thread_id: Some("thread-ui".into()),
            sources: vec![codex::ReferenceSource {
                message_id: "source".into(),
                role: "Assistant".into(),
            }],
            text: "/clear\ncode <literal>".into(),
        };
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input
                    .update(cx, |input, cx| input.set_value("preserved", window, cx));
                view.set_reference(Some(quote("old")), cx);
                view.submitted_reference = Some("old".into());
                view.submitted_ack = Some(7);
                view.submitted = Some("preserved".into());
                view.images = vec!["same-image".into()];
                view.submitted_images.clone_from(&view.images);
                view.set_reference(Some(quote("new")), cx);
                view.input.focus_handle(cx).focus(window, cx);
            });
        });
        let mut accepted = snapshot();
        accepted.accepted_submission = Some(7);
        updates.send(accepted).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.reference.as_ref().unwrap().id, "new");
            assert_eq!(view.input.read(cx).value().as_ref(), "preserved");
            assert_eq!(view.images, vec!["same-image"]);
        });
        assert!(cx.debug_bounds("codex-remove-reference").is_some());
        let mut focused = false;
        for _ in 0..20 {
            cx.simulate_keystrokes("tab");
            draw(cx);
            focused =
                cx.update(|window, cx| view.read(cx).reference_remove_focus.is_focused(window));
            if focused {
                break;
            }
        }
        assert!(focused, "Remove reference must be reachable by Tab");
        cx.simulate_keystrokes("enter");
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert!(view.reference.is_none());
            assert_eq!(view.input.read(cx).value().as_ref(), "preserved");
            assert_eq!(view.images, vec!["same-image"]);
        });
        assert!(cx.update(|window, cx| view.read(cx).input.focus_handle(cx).is_focused(window)));
    }

    #[gpui::test]
    fn repeated_message_copy_extends_feedback_and_resets_after_delay(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, _) = fixture(cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.copy_message("first".into(), "first text".into(), cx);
            });
        });
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(MOTION_SLOW_MS * 3));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.copy_message("second".into(), "second text".into(), cx);
            });
        });
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(MOTION_SLOW_MS * 3));
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.copied_message.as_ref().map(|(key, _)| key.as_str()),
                Some("second")
            );
        });
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|clip| clip.text())),
            Some("second text".into())
        );
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(MOTION_SLOW_MS * 3));
        cx.run_until_parked();
        view.read_with(cx, |view, _| assert!(view.copied_message.is_none()));
    }

    #[test]
    fn response_groups_split_only_at_users_and_copy_all_prose() {
        let items = vec![
            serde_json::json!({"type":"userMessage", "id":"u", "text":"exclude"}),
            serde_json::json!({"type":"agentMessage", "id":"first", "phase":"commentary", "text":"First", "groveReceivedAt":10}),
            serde_json::json!({"type":"commandExecution", "text":"exclude"}),
            serde_json::json!({"type":"reasoning", "text":"exclude"}),
            serde_json::json!({"type":"agentMessage", "id":"last", "phase":"final_answer", "text":"Last", "groveReceivedAt":20}),
            serde_json::json!({"type":"agentMessage", "id":"empty", "text":"  ", "groveReceivedAt":30}),
            serde_json::json!({"type":"userMessage", "id":"interrupt"}),
            serde_json::json!({"type":"agentMessage", "id":"next", "text":"Next"}),
            serde_json::json!({"type":"plan", "text":"exclude"}),
        ];
        let groups = response_groups(&items, Some("thread"));
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].key, "thread:response:first");
        assert_eq!(groups[0].copy_text, "First\n\nLast");
        assert_eq!(groups[0].received_at, Some(20));
        assert_eq!(groups[0].end, 6);
        assert_eq!(groups[1].end, items.len());
        assert_eq!(groups[1].received_at, None);
        assert!(response_groups(&items[2..4], None).is_empty());
        assert!(response_groups(&items[5..6], None).is_empty());
        let mut streaming = items[..5].to_vec();
        let key = response_groups(&streaming, Some("thread"))[0].key.clone();
        streaming
            .push(serde_json::json!({"type":"agentMessage", "id":"resumed", "text":"Resumed"}));
        let continued = response_groups(&streaming, Some("thread"));
        assert_eq!(continued[0].key, key);
        assert_eq!(
            continued[0].received_at, None,
            "Never borrow an earlier timestamp"
        );
        streaming.truncate(2);
        assert_eq!(
            response_groups(&streaming, Some("thread"))[0].copy_text,
            "First"
        );
    }

    #[gpui::test]
    fn response_footer_copies_collapsed_prose_and_survives_streaming(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, updates) = fixture(cx);
        let mut state = snapshot();
        state.saved.items = vec![
            serde_json::json!({"type":"agentMessage", "id":"first", "phase":"commentary", "text":"Hidden commentary"}),
            serde_json::json!({"type":"agentMessage", "id":"answer", "phase":"final_answer", "text":"Visible answer"}),
            serde_json::json!({"type":"commandExecution", "id":"tail", "status":"completed", "command":"true"}),
        ];
        updates.send(state.clone()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert!(cx.debug_bounds("codex-message-row-0").is_none());
        let footer = cx.debug_bounds("codex-response-footer").unwrap();
        let body = cx.debug_bounds("codex-message-row-1").unwrap();
        assert!(footer.origin.y >= body.bottom());
        let copy = cx.debug_bounds("codex-copy-response").unwrap().center();
        cx.simulate_click(copy, gpui::Modifiers::default());
        let key = view.read_with(cx, |view, _| {
            view.copied_message.as_ref().unwrap().0.clone()
        });
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|clip| clip.text())),
            Some("Hidden commentary\n\nVisible answer".into())
        );
        state.saved.items.push(serde_json::json!({"type":"agentMessage", "id":"resumed", "phase":"final_answer", "text":"Resumed answer"}));
        updates.send(state).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, _| view
                .copied_message
                .as_ref()
                .unwrap()
                .0
                .clone()),
            key
        );
        let copy = cx.debug_bounds("codex-copy-response").unwrap().center();
        cx.simulate_click(copy, gpui::Modifiers::default());
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|clip| clip.text())),
            Some("Hidden commentary\n\nVisible answer\n\nResumed answer".into())
        );
    }

    #[gpui::test]
    #[cfg(unix)]
    fn codex_and_claude_snippet_run_selects_owned_terminal_and_preserves_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let root = std::env::temp_dir().join(format!(
                "grove-snippet-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs_err::create_dir_all(&root).unwrap();
            assert!(std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(&root)
                .status()
                .unwrap()
                .success());
            let path = root.canonicalize().unwrap().to_string_lossy().into_owned();
            let (view, cx, _actions, updates) = fixture(cx);
            let runtime = cx.update(|window, cx| {
                cx.set_global(crate::settings::SettingsState::new(
                    grove_core::storage::Store {
                        projects: vec![grove_core::storage::Project {
                            name: "Snippet fixture".into(),
                            path: path.clone(),
                            scripts: grove_core::storage::ProjectScripts::default(),
                            archived: false,
                            worktree_dir: None,
                        }],
                        ..Default::default()
                    },
                ));
                cx.set_global(crate::zoom::CurrentPtyDims::default());
                let runtime = cx.new(crate::runtime::Runtime::new);
                view.update(cx, |view, cx| {
                    view.set_script_runtime(runtime.clone(), cx);
                    view.input
                        .update(cx, |input, cx| input.set_value("unsent draft", window, cx));
                });
                runtime
            });
            let code = "printf '%s\n' ' first ' > snippet-output.txt\nprintf '%s\n' 'second' >> snippet-output.txt\npwd > snippet-cwd.txt";
            let mut response = snapshot();
            response.saved.provider = provider;
            response.saved.cwd = path.clone();
            response.saved.items.push(json!({"type":"agentMessage","id":"shell-answer","text":format!("```sh\n{code}\n```"),"groveCompleted":true}));
            updates.send(response).unwrap();
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(40));
            draw(cx);
            let run = cx.debug_bounds("codex-run-code").unwrap().center();
            // Dispatch the native click without polling the deterministic executor:
            // a real PTY must not wake TestDispatcher from its bridge thread.
            cx.update(|window, cx| {
                use gpui::InputEvent as _;
                window.dispatch_event(
                    gpui::MouseDownEvent {
                        position: run,
                        button: gpui::MouseButton::Left,
                        modifiers: gpui::Modifiers::default(),
                        click_count: 1,
                        first_mouse: false,
                    }
                    .to_platform_input(),
                    cx,
                );
                window.dispatch_event(
                    gpui::MouseUpEvent {
                        position: run,
                        button: gpui::MouseButton::Left,
                        modifiers: gpui::Modifiers::default(),
                        click_count: 1,
                    }
                    .to_platform_input(),
                    cx,
                );
            });
            cx.update(|_, cx| {
                let registry = runtime.read(cx).registry.read(cx);
                let sessions = registry.all();
                assert_eq!(sessions.len(), 1);
                assert_eq!(sessions[0].wt_path, path);
                assert_eq!(sessions[0].agent, grove_core::agent::Agent::Terminal);
                assert_eq!(
                    runtime.read(cx).state.read(cx).active_session(),
                    Some(sessions[0].id)
                );
                assert_eq!(
                    view.read(cx).input.read(cx).value().as_ref(),
                    "unsent draft"
                );
            });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while fs_err::read_to_string(root.join("snippet-cwd.txt"))
                .ok()
                .is_none_or(|cwd| cwd.trim() != path)
            {
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert_eq!(
                fs_err::read_to_string(root.join("snippet-output.txt")).unwrap(),
                " first \nsecond\n"
            );
            assert_eq!(
                fs_err::read_to_string(root.join("snippet-cwd.txt"))
                    .unwrap()
                    .trim(),
                path
            );
            cx.update(|_, cx| {
                let registry = runtime.read(cx).registry.clone();
                let ids = registry
                    .read(cx)
                    .all()
                    .iter()
                    .map(|meta| meta.id)
                    .collect::<Vec<_>>();
                registry.update(cx, |registry, _| {
                    for id in ids {
                        registry.remove(id);
                    }
                });
            });
            fs_err::remove_dir_all(root).unwrap();
        }
    }

    #[gpui::test]
    fn agent_code_blocks_and_finished_responses_copy_verbatim(cx: &mut gpui::TestAppContext) {
        let (view, cx, _actions, updates) = fixture(cx);
        let text = "Run this:\n\n```sh\n  cargo test --workspace\n```";
        let mut response = snapshot();
        response
            .saved
            .items
            .push(serde_json::json!({"type":"agentMessage", "id":"answer", "text":text, "groveCompleted":true}));
        updates.send(response).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        let clipboard = |cx: &mut gpui::VisualTestContext| {
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|clip| clip.text()))
        };
        assert!(cx.debug_bounds("codex-run-code").is_some(), "Finalized shell fences retain a disabled Run control when checkout runtime is unavailable");
        let copy_bounds = cx.debug_bounds("codex-copy-code").unwrap();
        cx.simulate_click(copy_bounds.center(), gpui::Modifiers::default());
        draw(cx);
        assert_eq!(cx.debug_bounds("codex-code-copied"), Some(copy_bounds));
        assert_eq!(
            view.read_with(cx, |view, _| view.copied_code_blocks.len()),
            1
        );
        assert_eq!(
            clipboard(cx).as_deref().map(str::trim_end),
            Some("  cargo test --workspace")
        );
        let response = cx.debug_bounds("codex-copy-response").unwrap().center();
        cx.simulate_click(response, gpui::Modifiers::default());
        assert_eq!(clipboard(cx).as_deref(), Some(text));
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(CODE_COPY_FEEDBACK_MS));
        cx.run_until_parked();
        draw(cx);
        assert!(cx.debug_bounds("codex-code-copied").is_none());
        assert_eq!(cx.debug_bounds("codex-copy-code"), Some(copy_bounds));
    }

    #[gpui::test]
    fn snippet_copy_keyboard_shows_feedback(cx: &mut gpui::TestAppContext) {
        let (view, cx, _, updates) = fixture_with_navigation(cx, true);
        let mut state = snapshot();
        state.saved.items.push(json!({"type":"agentMessage", "id":"keyboard", "text":"```rust\n  let value = 1;\n```", "groveCompleted":true}));
        updates.send(state).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        cx.update(|window, cx| {
            let focus = view.read(cx).message_focus["thread-ui:keyboard:code:0"].clone();
            focus.focus(window, cx);
        });
        draw(cx);
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(
            cx.debug_bounds("codex-code-copied").is_some(),
            "Enter must activate focused snippet Copy"
        );
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
            Some("  let value = 1;".into())
        );
    }

    #[gpui::test]
    fn snippet_copy_feedback_is_local_and_repeated_click_extends_timeout(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _, updates) = fixture(cx);
        let mut state = snapshot();
        state.saved.provider = codex::Provider::Claude;
        state.saved.items.push(json!({"type":"agentMessage", "id":"answer", "text":"```rust\nlet first = 1;\n```\n\n```rust\nlet second = 2;\n```", "groveCompleted":true}));
        updates.send(state).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        let first = cx.debug_bounds("codex-copy-code").unwrap();
        cx.simulate_click(first.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        draw(cx);
        assert_eq!(cx.debug_bounds("codex-code-copied"), Some(first));
        let first_payload =
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
        let other = cx.debug_bounds("codex-copy-code").unwrap();
        assert_ne!(other, first, "other block must still offer Copy");
        cx.simulate_click(other.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.copied_code_blocks.len()),
            2
        );
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(CODE_COPY_FEEDBACK_MS / 2));
        cx.simulate_click(first.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(CODE_COPY_FEEDBACK_MS / 2));
        cx.run_until_parked();
        draw(cx);
        assert_eq!(cx.debug_bounds("codex-code-copied"), Some(first));
        assert_eq!(cx.debug_bounds("codex-copy-code"), Some(other));
        assert_eq!(
            view.read_with(cx, |view, _| view.copied_code_blocks.len()),
            1
        );
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(CODE_COPY_FEEDBACK_MS / 2));
        cx.run_until_parked();
        draw(cx);
        assert!(cx.debug_bounds("codex-code-copied").is_none());
        assert!(view.read_with(cx, |view, _| view.copied_code_blocks.is_empty()));
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
            first_payload
        );
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
        assert!(view.read_with(cx, |view, _| {
            view.images[0].starts_with("data:image/png;base64,")
        }));
        cx.update(|window, cx| view.update(cx, |view, cx| view.submit(window, cx)));
        assert!(actions.try_iter().any(|action| matches!(action, Action::Submit {text,images,..} if text.trim()=="Image 1 inspect screenshot" && images.len()==1 && images[0].starts_with("data:image/png;base64,"))));
    }
}

#[cfg(test)]
mod transcript_signature_tests {
    use super::transcript_content_signature;
    use serde_json::json;

    #[test]
    fn content_signature_tracks_equal_length_edits_optimistic_items_images_and_threads() {
        let mut items = vec![json!({"id":"answer", "type":"agentMessage", "text":"one"})];
        let original = transcript_content_signature(&items, Some("thread-a"));
        assert_eq!(
            original,
            transcript_content_signature(&items, Some("thread-a"))
        );
        items[0]["text"] = json!("two");
        let replaced = transcript_content_signature(&items, Some("thread-a"));
        assert_ne!(original, replaced);
        items.push(json!({"id":"optimistic", "type":"userMessage", "text":"next"}));
        let optimistic = transcript_content_signature(&items, Some("thread-a"));
        assert_ne!(replaced, optimistic);
        items[1]["images"] = json!(["data:image/png;base64,AAAA"]);
        let image = transcript_content_signature(&items, Some("thread-a"));
        assert_ne!(optimistic, image);
        items[1]["images"] = json!(["data:image/png;base64,BBBB"]);
        let edited_image = transcript_content_signature(&items, Some("thread-a"));
        assert_ne!(image, edited_image);
        assert_ne!(
            edited_image,
            transcript_content_signature(&items, Some("thread-b"))
        );
    }
}
