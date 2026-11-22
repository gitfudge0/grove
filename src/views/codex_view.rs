//! Native conversation surface for a structured Codex session.
mod composer_suggestions;

mod rewind;

mod attachments;

use super::status::{self, StatusKind};
use super::{
    components::{
        attached_validation, form_action, header_control, header_control_frame,
        header_control_with_label_tooltip, header_control_with_tooltip, preference_choices,
        project_field_well, question_choice, settings_control, shimmer_label, task_mark,
        task_meter, tooltip, tooltip_content, transcript_action, truncation_safe_label_width,
        TaskPhase,
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
        IndentInline, Input, InputEvent, InputState, MoveDown, MoveLeft, MoveRight, MoveUp,
        TextDecoration, TextDecorationCollection, TextReplacement,
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

mod activity_request;
mod approval_cards;
mod elicitation_cards;
mod question_cards;
pub(crate) use activity_request::{activity_request, ActivityRequest, ActivityRequestTarget};
mod composer_options;
mod dashboard_status;
mod history;
mod measured_slot;
mod measured_tool;
mod message_rows;
mod plan_composer;
mod queued_prompts;
mod snippets;
mod submission;
mod transcript_cache;
use history::TurnHistory;
use transcript_cache::{
    PreparedItem, PreparedTranscript, RichMessageIndex, TranscriptRenderCache, WorkIndex,
};
#[path = "model_picker.rs"]
mod model_picker;
#[cfg(test)]
mod table_sizing_tests;

const CODE_COPY_FEEDBACK_MS: u64 = 1200;
const NEXT_REPLY_POSITION_W: f32 = 32.0;
const STOP_CONFIRMATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

struct StopConfirmation {
    thread_id: Option<String>,
    turn_id: String,
    started: std::time::Instant,
}

/// Paint Markdown source without replacing text or changing editor mode. Leaf
/// spans are ordered and disjoint, so nested emphasis composes correctly with
/// independent picked-file/image decorations and UTF-8 cursor offsets.
pub(super) fn composer_markdown_decorations(text: &str) -> Vec<TextDecoration> {
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
                style.color = Some(c::FG_DIM());
                style.underline = Some(gpui::UnderlineStyle {
                    thickness: gpui::px(1.),
                    ..Default::default()
                });
            }
            Node::InlineCode(_) => {
                // Match TextView inline code: neutral backplate, medium weight, no outline.
                style.background_color = Some(c::MENU_HOVER());
                style.font_weight = Some(gpui::FontWeight::MEDIUM);
            }
            Node::Code(_) => {
                style.background_color = Some(c::CODE_FILL());
                style.color = Some(c::FG());
                style.font_weight = Some(gpui::FontWeight::NORMAL);
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
                    let decoration = TextDecoration::new(range, style);
                    out.push(if matches!(node, Node::Code(_) | Node::InlineCode(_)) {
                        decoration.font_family(crate::fonts::MONO_FAMILY)
                    } else {
                        decoration
                    });
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
pub(super) fn composer_markdown_replacements(text: &str) -> Vec<TextReplacement> {
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

/// Process-local change identity. TranscriptItems invalidates its revision before
/// every mutable borrow, including equal-length edits to opaque image payloads.
/// No provider payload is traversed to decide whether bottom-follow should run.
fn transcript_content_signature(items: &codex::TranscriptItems, thread: Option<&str>) -> [u64; 2] {
    let mut hasher = rustc_hash::FxHasher::default();
    thread.hash(&mut hasher);
    [items.revision(), hasher.finish()]
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

/// Bottom of the session composer, including its canvas inset and hint row.
/// The wide side conversation shares this baseline rather than the pane edge.
pub(super) fn composer_bottom_inset(window: &Window) -> gpui::Pixels {
    rpx(SPACE_3XL * 2.0 + SPACE_MD).to_pixels(window.rem_size())
        + gpui::phi()
            .to_pixels(rpx(TEXT_MICRO).into(), window.rem_size())
            .round()
}

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
/// Keep the review dock an overview; detailed evidence stays in the transcript.
fn review_overview(response: &str) -> String {
    super::review_summary::ReviewSummary::parse(response, std::path::Path::new("/")).text
}

/// The task strip header uses the compact control height.
const TASK_STRIP_HEAD_H: f32 = CONTROL_H;

/// One task row in the expanded strip (product/design.md row-h).
const TASK_ROW_H: f32 = CONTROL_H + SPACE_SM;
/// Rows reserve the header disclosure column before their status mark.
const TASK_MARK_INSET: f32 = ICON_MD + SPACE_SM;
const TASK_LABEL_INSET: f32 = TASK_MARK_INSET + ICON_SM + SPACE_LG;
/// One full turn of the running task mark.
const TASK_SPINNER_PERIOD: std::time::Duration =
    std::time::Duration::from_millis(MOTION_SLOW_MS * 4);
/// Clock ticks per spinner frame, the inverse of `animation_clock::spinner_frame`.
const SPINNER_TICKS_PER_FRAME: u64 = 3;

/// The minimap has a definite pane-sized host, so unchanged scenes can be replayed
/// independently of transcript animation and scroll frames.
struct TranscriptMinimap {
    parent: gpui::WeakEntity<CodexView>,
    prepared: Rc<PreparedTranscript>,
    active: Option<usize>,
    preview_width: gpui::Pixels,
    colors: [gpui::Hsla; 6],
}
impl Render for TranscriptMinimap {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.parent
            .update(cx, |parent, cx| {
                parent.build_message_minimap(
                    &self.prepared.minimap_entries,
                    self.active,
                    self.preview_width,
                    window,
                    cx,
                )
            })
            .unwrap_or_else(|_| div().into_any_element())
    }
}
#[cfg(test)]
thread_local! { static MINIMAP_BUILDS: Cell<usize> = const { Cell::new(0) }; }

/// A user input and the final (or latest streaming) assistant message before
/// the next input. Source indices stay distinct from rendered child indices.
#[derive(Clone, Debug, PartialEq)]
struct MessageMinimapEntry {
    item_index: usize,
    prompt: String,
    response: String,
    prompt_preview: gpui::SharedString,
    response_preview: gpui::SharedString,
    label: gpui::SharedString,
    description: gpui::SharedString,
}

/// Assistant review metadata belongs to the review panel; retain raw saved items.
fn transcript_item_text(item: &serde_json::Value) -> String {
    let text = codex::item_text(item);
    if item["type"] == "agentMessage" {
        grove_core::review_brief::display_text(&text)
    } else {
        text
    }
}

#[cfg(test)]
fn message_minimap_entries<T: std::borrow::Borrow<serde_json::Value>>(
    items: &[T],
) -> Vec<MessageMinimapEntry> {
    message_minimap_entries_from(items, 0, None)
}

/// Preparation already extracted lossless display text. Borrow it instead of
/// repeatedly cloning large provider strings while deriving groups and excerpts.
fn prepared_item_text<'a, T: std::borrow::Borrow<serde_json::Value>>(
    items: &'a [T],
    prepared: Option<&'a [Rc<PreparedItem>]>,
    index: usize,
) -> std::borrow::Cow<'a, str> {
    prepared.map_or_else(
        || std::borrow::Cow::Owned(transcript_item_text(items[index].borrow())),
        |prepared| std::borrow::Cow::Borrowed(prepared[index].text.as_ref()),
    )
}

fn message_minimap_entries_from<T: std::borrow::Borrow<serde_json::Value>>(
    items: &[T],
    restart: usize,
    prepared: Option<&[Rc<PreparedItem>]>,
) -> Vec<MessageMinimapEntry> {
    let inputs: Vec<_> = items
        .iter()
        .enumerate()
        .skip(restart)
        .filter(|(_, item)| item.borrow()["type"] == "userMessage")
        .map(|(index, _)| index)
        .collect();
    inputs
        .iter()
        .enumerate()
        .map(|(turn, &index)| {
            let end = inputs.get(turn + 1).copied().unwrap_or(items.len());
            let mut latest = None;
            let mut final_answer = None;
            for reply in index + 1..end {
                if items[reply].borrow()["type"] == "agentMessage"
                    && !prepared_item_text(items, prepared, reply).trim().is_empty()
                {
                    latest = Some(reply);
                    if items[reply].borrow()["phase"] == "final_answer" {
                        final_answer = Some(reply);
                    }
                }
            }
            let response = final_answer.or(latest).map_or_else(
                || "No response yet".into(),
                |reply| minimap_source_excerpt(&prepared_item_text(items, prepared, reply)),
            );
            let prompt = prepared_item_text(items, prepared, index);
            let prompt = if prompt.trim().is_empty() {
                "Image attachment".into()
            } else {
                minimap_source_excerpt(&prompt)
            };
            MessageMinimapEntry {
                item_index: index,
                prompt_preview: minimap_excerpt(&prompt).into(),
                response_preview: minimap_excerpt(&response).into(),
                label: format!("Go to message: {prompt}").into(),
                description: format!("Response: {response}").into(),
                prompt,
                response,
            }
        })
        .collect()
}

fn minimap_source_excerpt(text: &str) -> String {
    let mut characters = text.chars();
    let mut preview = characters.by_ref().take(240).collect::<String>();
    if characters.next().is_some() {
        preview.push('…');
    }
    preview
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
pub(super) fn copy_control(
    id: &'static str,
    label: &'static str,
    text: impl Into<gpui::SharedString>,
) -> gpui::Stateful<gpui::Div> {
    let text = text.into();
    let keyboard = text.clone();
    header_control_with_label_tooltip(id, label)
        .debug_selector(move || id.into())
        .tab_index(0)
        .focus_visible(|style| style.bg(c::BG_HOVER()))
        .child(crate::icons::icon("copy", ICON_MD, c::FG_DIM()))
        .on_key_down(move |event, _, cx| {
            if action_key(event) {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(keyboard.to_string()));
                cx.stop_propagation();
            }
        })
        .on_click(move |_, _, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.to_string()));
        })
}

pub(super) fn composer_icon(
    id: &'static str,
    label: &'static str,
    icon: &'static str,
    disabled: bool,
    primary: bool,
) -> gpui::Stateful<gpui::Div> {
    let fill = if icon == "stop" { c::RED() } else { c::FG() };
    super::components::inline_disclosure(id, label)
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
            button
                .group("disabled-control")
                .cursor_default()
                .opacity(OPACITY_DISABLED)
        })
        .child(crate::icons::icon(
            icon,
            ICON_SM,
            if primary { c::BG() } else { c::FG_DIM() },
        ))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ComposerMenu {
    Add,
    Settings,
    Model,
    Effort,
    Access,
}

struct ClosingComposerPopup {
    menu: ComposerMenu,
    popup: Option<Entity<PopupMenu>>,
    model_picker: Option<Entity<model_picker::ModelPicker>>,
    bounds: gpui::Bounds<gpui::Pixels>,
    scope: String,
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

pub(super) fn composer_model_name(state: &codex::Snapshot) -> String {
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

// These labels must retain their full text. In particular, do not feed an
// explicitly measured width back into GPUI's independent ellipsis measurement.
fn full_model_label(text: gpui::StyledText) -> gpui::Div {
    div()
        .text_size(rpx(TEXT_SMALL))
        .flex_shrink_0()
        .whitespace_nowrap()
        .child(text)
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
    activity_anchor: activity_request::RequestAnchor,
    fields: Vec<ElicitationFieldDraft>,
    submitting: bool,
}

struct ApprovalDraft {
    activity_anchor: activity_request::RequestAnchor,
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
    activity_anchor: activity_request::RequestAnchor,
    request_instance: u64,
    reviewing: bool,
    row_anchors: Vec<gpui::ScrollAnchor>,
    questions: Vec<QuestionDraft>,
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

/// Pending inputs keep their submission position while provider events stream.
/// This is only a render projection; saved items and their work ranges stay intact.
fn project_pending_bubbles(state: &mut codex::Snapshot, optimistic: &[serde_json::Value]) {
    let mut positions = optimistic
        .iter()
        .enumerate()
        .map(|(order, item)| {
            let position = match item.get("localAfterItem") {
                Some(anchor) => anchor
                    .as_str()
                    .and_then(|id| {
                        state
                            .saved
                            .items
                            .iter()
                            .position(|old| old["id"] == id)
                            .map(|at| at + 1)
                    })
                    .unwrap_or(0), // Compaction may remove the anchor; never move input below its reply.
                None => state.saved.items.len(),
            };
            (position, order, item)
        })
        .collect::<Vec<_>>();
    // Reverse insertion keeps sends sharing an anchor in submission order.
    positions.sort_by_key(|(position, order, _)| (*position, *order));
    for (position, _, item) in positions.into_iter().rev() {
        state.saved.items.insert(position, item.clone().into());
        for period in &mut state.saved.work_periods {
            if period.start_item > position {
                period.start_item += 1;
            }
            if let Some(end) = &mut period.end_item {
                if *end > position {
                    *end += 1;
                }
            }
        }
    }
}

/// A local send can be acknowledged before its transcript item arrives.
struct BubbleEntrance {
    submission: u64,
    existing: HashSet<String>,
    thread: Option<String>,
    text: String,
    optimistic: Option<serde_json::Value>,
}

struct PlanAnswerComposer {
    plan_id: String,
    revision_id: String,
    decision_id: String,
    prompt: String,
    thread_id: Option<String>,
    draft: String,
    images: Vec<String>,
    tokens: Vec<ComposerToken>,
    reference: Option<codex::ReferenceDraft>,
    saving: Option<String>,
    error_before_save: Option<String>,
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

#[derive(Clone)]
struct RewindConfirmation {
    index: usize,
    message_id: String,
    thread_id: Option<String>,
    source_bounds: Rc<Cell<gpui::Bounds<gpui::Pixels>>>,
    source_anchor: gpui::ScrollAnchor,
    source_focus: FocusHandle,
    files_unavailable: Option<String>,
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
    DashboardSubmit { open_transcript: bool },
    OpenConnections(codex::Provider, String, Vec<String>, Option<FocusHandle>),
    DescribeChanges(grove_core::describe_skill::ScopePreference, FocusHandle),
    ReviewChanges(grove_core::review_skill::ScopePreference, FocusHandle),
    ForkRequested(usize, bool),
    FileOpened,
    FileClosed,
}
impl gpui::EventEmitter<CodexViewEvent> for CodexView {}

pub struct CodexView {
    pub(crate) dashboard_tray: Option<Entity<super::sidebar::dashboard::DestinationTray>>,
    dashboard_pending: bool,
    dashboard_handoffs: Vec<(gpui::WeakEntity<CodexSession>, Subscription)>,
    dashboard_status_expanded: bool,
    dashboard_status_signature: Vec<dashboard_status::Preparation>,
    dashboard_more_focus: FocusHandle,
    dashboard_tray_observer: Option<Subscription>,
    dashboard_retry_focus: [FocusHandle; 2],
    script_runtime: Option<Entity<crate::runtime::Runtime>>,
    preview_child: Option<String>,
    history_readonly: bool,
    session: Entity<CodexSession>,
    pub(super) html: Entity<super::inline_html::HtmlSession>,
    pub(super) file_browser: Entity<super::file_browser::FileBrowser>,
    markdown_decorations: TextDecorationCollection,
    markdown_source: Option<(String, u64, Vec<TextDecoration>)>,
    next_reply_seen: Option<String>,
    next_reply_request: Option<String>,
    next_reply: grove_core::next_reply::Candidate,
    next_reply_control: Option<grove_core::next_reply::JobControl>,
    next_reply_task: Option<gpui::Task<()>>,
    next_reply_focus: [FocusHandle; 2],
    next_reply_generate: fn(
        &grove_core::next_reply::Request,
        &grove_core::commit_agent::CommitControl,
    ) -> Result<Vec<String>, String>,
    plan_answer: Option<PlanAnswerComposer>,
    native_plans: HashMap<(String, String), Entity<super::native_plan::NativePlan>>,
    pub(crate) description_scope: Option<grove_core::change_description::Scope>,
    pub(crate) review_scope: Option<grove_core::change_description::Scope>,
    input: Entity<InputState>,
    reference: Option<codex::ReferenceDraft>,
    submitted_reference: Option<String>,
    reference_expanded: bool,
    reference_disclosure_focus: FocusHandle,
    reference_remove_focus: FocusHandle,
    selection_views: HashMap<String, Entity<TextViewState>>,
    selection_subscriptions: HashMap<String, Subscription>,
    focus_subscriptions: HashMap<String, (Subscription, Subscription)>,
    focus_revision: Option<u64>,
    focus_thread: Option<String>,
    focus_sources: HashMap<String, codex::TranscriptItem>,
    message_slots:
        rustc_hash::FxHashMap<String, Rc<std::cell::RefCell<measured_slot::Measurement>>>,
    tool_slots: rustc_hash::FxHashMap<String, Rc<std::cell::RefCell<measured_tool::Measurement>>>,
    work_header_slots:
        rustc_hash::FxHashMap<String, Rc<std::cell::RefCell<measured_slot::HeaderMeasurement>>>,
    selection_dismissed: Option<TextSelectionSnapshot>,
    selection_root_observed: bool,
    selection_reply_focus: FocusHandle,
    submitted_images: Vec<String>,
    pub(super) transcript_bounds: Rc<Cell<gpui::Bounds<gpui::Pixels>>>,
    pub(super) composer_rail_scroll: gpui::ScrollHandle,
    checkout_context: Option<(String, String)>,
    expanded: HashSet<String>,
    pub(super) agents_in_checkout: bool,
    work_tick_pending: Rc<Cell<bool>>,
    question_drafts: HashMap<String, QuestionGroupDraft>,
    question_receipts: Vec<QuestionReceipt>,
    question_generation: u64,
    approval_drafts: HashMap<String, ApprovalDraft>,
    elicitation_drafts: HashMap<String, ElicitationDraft>,
    activity_request_focus: Option<activity_request::PendingRequestFocus>,
    agent_controls_scroll: gpui::ScrollHandle,
    agent_controls_anchor: gpui::ScrollAnchor,
    interaction_scope: String,
    menu: Option<ComposerMenu>,
    popup_motion: super::motion::PopupMotion,
    closing_popup: Option<ClosingComposerPopup>,
    popup_generation: u64,
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
    option_bounds: [Rc<Cell<gpui::Bounds<gpui::Pixels>>>; 4],
    option_focus: [FocusHandle; 4],
    file_suggestions: Vec<String>,
    picked_tokens: Vec<ComposerToken>,
    suggestion_index: usize,
    suggestion_scroll: gpui::ScrollHandle,
    file_query: String,
    file_lookup: Option<String>,
    dismissed_suggestions: Option<String>,
    submitted: Option<String>,
    submitted_ack: Option<u64>,
    pending_clear: Option<(u64, String, Vec<String>)>,
    edit_confirmation: Option<RewindConfirmation>,
    message_action_bounds: HashMap<String, Rc<Cell<gpui::Bounds<gpui::Pixels>>>>,
    edit_request_pending: bool,
    rewind_attempt: Option<u64>,
    edited_messages: u64,
    edit_cancel_focus: FocusHandle,
    edit_confirm_focus: FocusHandle,
    edit_keep_focus: FocusHandle,
    queue_resume_focus: FocusHandle,
    queue_selected: Option<u64>,
    queue_collapsed: bool,
    queue_editor: Option<QueueEditor>,
    queue_motion_rows: Vec<QueueMotionRow>,
    bubble_entrances: Vec<BubbleEntrance>,
    animating_bubbles: HashMap<String, (std::time::Instant, Rc<Cell<bool>>)>,
    interrupt_requested_for_turn: Option<String>,
    stop_confirmation: Option<StopConfirmation>,
    stop_confirmation_timer: Option<gpui::Task<()>>,
    conversation_focus: FocusHandle,
    images: Vec<String>,
    previews: HashMap<String, Arc<gpui::Image>>,
    transcript_previews:
        rustc_hash::FxHashMap<(usize, usize), (codex::TranscriptItem, Arc<gpui::Image>)>,
    pending_image_pastes: usize,
    pending_attachment_picker: bool,
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
    message_focus: rustc_hash::FxHashMap<String, FocusHandle>,
    copied_message: Option<(String, u64)>,
    copied_code_blocks: HashMap<String, u64>,
    snippets: HashMap<String, snippets::Snippet>,
    preview_snapshot: Option<(Rc<codex::Snapshot>, String, Rc<codex::Snapshot>)>,
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
    status_card_keys: HashSet<String>,
    task_cache: Option<Rc<TaskCache>>,
    transcript_cache: Option<Rc<TranscriptRenderCache>>,
    minimap_view: Option<Entity<TranscriptMinimap>>,
    snippet_sources: std::cell::RefCell<snippets::SnippetSources>,
    turn_history: TurnHistory,
    history_earlier_focus: FocusHandle,
    history_latest_focus: FocusHandle,
    pending_history_jump: Option<usize>,
    subscriptions: Vec<Subscription>,
}
impl CodexView {
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

    /// Render the original retained transcript with disclosure and Copy only.
    pub fn history(
        session: Entity<CodexSession>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_history_mode(session, true, window, cx)
    }

    fn transcript_only(&self) -> bool {
        self.history_readonly || self.preview_child.is_some()
    }

    pub fn new(session: Entity<CodexSession>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_history_mode(session, false, window, cx)
    }

    fn new_with_history_mode(
        session: Entity<CodexSession>,
        history_readonly: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
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
                .accept_file_drop(true)
                .markdown_auto_continue(true)
                .submit_on_enter(true)
                .placeholder(placeholder)
                .default_value(draft)
        });
        let markdown_decorations = input.update(cx, |input, cx| {
            input.create_decorations_collection(Vec::new(), cx)
        });
        let card_focus = cx.focus_handle();
        let conversation_focus = cx.focus_handle().tab_stop(false);
        let mut subscriptions = vec![
            cx.observe_in(&session, window, |this, _, window, cx| {
                this.validate_stop_confirmation(cx);
                this.flush_dashboard_submission(window, cx);
                this.sync_native_plans(window, cx);
                this.sync_plan_answer(window, cx);
                if this.menu.is_some() && !this.composer_menu_valid(cx) {
                    this.close_menu(cx);
                }
                if let Some(picker) = &this.model_picker {
                    let session = this.session.read(cx);
                    if session.catalog_missing_executable.is_some() {
                        let provider = session.state.saved.provider;
                        picker.update(cx, |picker, cx| {
                            picker.mark_missing(provider, cx);
                            picker.discover(cx);
                        });
                    }
                }
                cx.notify();
            }),
            cx.observe(&input, |this, _, cx| {
                this.validate_picked_tokens(cx);
                this.refresh_files(cx);
                cx.notify();
            }),
            cx.subscribe_in(&input, window, |this, _, event, window, cx| {
                if this.transcript_only() {
                    return;
                }
                if this.dashboard_pending {
                    // The launched first message belongs to the handoff until send or
                    // failure. An opened conversation must not edit that submission.
                    if matches!(event, InputEvent::Change) {
                        let original = this.session.read(cx).state.saved.draft.clone();
                        if this.input.read(cx).value().as_ref() != original {
                            this.input
                                .update(cx, |input, cx| input.set_value(original, window, cx));
                        }
                    }
                    return;
                }
                match event {
                    InputEvent::PressEnter {
                        secondary: true,
                        shift: false,
                    } => this.submit_modified_enter(true, window, cx),
                    InputEvent::PressEnter {
                        secondary: false,
                        shift: false,
                    } => this.submit(window, cx),
                    InputEvent::DropFiles(paths) => {
                        this.attach_paths(paths.clone(), window, cx);
                    }
                    InputEvent::PasteImages(images) => {
                        this.paste_images(images.clone(), window, cx);
                    }
                    InputEvent::Focus => {
                        crate::platform::inline_html::NativeDocument::focus_host(window);
                        this.set_composer_collapsed(false, cx);
                        cx.notify();
                    }
                    InputEvent::Blur => cx.notify(),
                    InputEvent::Change => {
                        this.clear_stop_confirmation(cx);
                        if this.plan_answer.is_some() {
                            this.persist_plan_answer_draft(cx);
                            this.sync_plan_composer_pending(cx);
                            cx.notify();
                            return;
                        }
                        this.set_composer_collapsed(false, cx);
                        let text = this.input.read(cx).value().to_string();
                        if !this.transcript_only()
                            && this.queue_editor.is_none()
                            && !this.edit_request_pending
                            && this.session.read(cx).state.saved.pending_edit.is_none()
                        {
                            this.queue_selected = None;
                            this.session
                                .update(cx, |session, cx| session.send(Action::Draft(text), cx));
                        }
                        this.dismissed_suggestions = None;
                        this.suggestion_index = 0;
                        this.suggestion_scroll.scroll_to_item(0);
                        this.validate_picked_tokens(cx);
                        this.refresh_files(cx);
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => {}
                }
            }),
        ];
        let scroll = gpui::ScrollHandle::new();
        let html_scroll = scroll.clone();
        let cwd = std::path::PathBuf::from(&session.read(cx).state.saved.cwd);
        let file_browser =
            cx.new(|cx| super::file_browser::FileBrowser::new(cwd.clone(), window, cx));
        subscriptions.push(cx.observe(&file_browser, |_, _, cx| cx.notify()));
        subscriptions.push(cx.subscribe(&file_browser, |this, _, event, cx| {
            match event {
                super::file_browser::FileBrowserEvent::Opened => {
                    this.close_menu(cx);
                    cx.emit(CodexViewEvent::FileOpened);
                }
                super::file_browser::FileBrowserEvent::Closed => {
                    cx.emit(CodexViewEvent::FileClosed);
                }
            }
            cx.notify();
        }));
        let html = cx.new(|_| super::inline_html::HtmlSession::new(cwd, html_scroll));
        subscriptions.push(cx.subscribe_in(
            &html,
            window,
            |this, _, event: &super::inline_html::OpenLocalFile, window, cx| {
                this.file_browser
                    .update(cx, |browser, cx| browser.open(event.0.clone(), window, cx));
                cx.notify();
            },
        ));
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
        subscriptions.push(
            cx.on_focus_out(&conversation_focus, window, |this, _, _, cx| {
                this.clear_stop_confirmation(cx);
            }),
        );
        let next_reply_seen =
            grove_core::next_reply::Request::capture(&session.read(cx).state).map(|r| r.key);
        let composer_rail_scroll = gpui::ScrollHandle::new();
        let mut view = Self {
            preview_child: None,
            history_readonly,
            session,
            html,
            file_browser,
            plan_answer: None,
            native_plans: HashMap::new(),
            description_scope: None,
            review_scope: None,
            markdown_decorations,
            markdown_source: None,
            next_reply_seen,
            next_reply_request: None,
            next_reply: grove_core::next_reply::Candidate::pending(),
            next_reply_control: None,
            next_reply_task: None,
            next_reply_focus: std::array::from_fn(|_| cx.focus_handle()),
            next_reply_generate: grove_core::next_reply::Request::generate,
            input,
            reference,
            submitted_reference: None,
            reference_expanded: false,
            reference_disclosure_focus: cx.focus_handle(),
            reference_remove_focus: cx.focus_handle(),
            selection_views: HashMap::new(),
            selection_subscriptions: HashMap::new(),
            focus_subscriptions: HashMap::new(),
            focus_revision: None,
            focus_thread: None,
            focus_sources: HashMap::new(),
            message_slots: rustc_hash::FxHashMap::default(),
            tool_slots: rustc_hash::FxHashMap::default(),
            work_header_slots: rustc_hash::FxHashMap::default(),
            selection_dismissed: None,
            selection_root_observed: false,
            selection_reply_focus: cx.focus_handle(),
            submitted_images: Vec::new(),
            transcript_bounds: Rc::default(),
            composer_rail_scroll: composer_rail_scroll.clone(),
            expanded: HashSet::new(),
            agents_in_checkout: false,
            work_tick_pending: Rc::new(Cell::new(false)),
            question_drafts: HashMap::new(),
            question_receipts: Vec::new(),
            question_generation: 0,
            approval_drafts: HashMap::new(),
            elicitation_drafts: HashMap::new(),
            activity_request_focus: None,
            agent_controls_scroll: gpui::ScrollHandle::new(),
            agent_controls_anchor: gpui::ScrollAnchor::for_handle(composer_rail_scroll.clone()),
            interaction_scope: String::new(),
            menu: None,
            popup_motion: super::motion::PopupMotion::default(),
            closing_popup: None,
            popup_generation: 0,
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
            suggestion_scroll: gpui::ScrollHandle::new(),
            file_query: String::new(),
            file_lookup: None,
            dismissed_suggestions: None,
            submitted: None,
            submitted_ack: None,
            pending_clear: None,
            message_action_bounds: HashMap::new(),
            edit_confirmation: None,
            edit_request_pending: false,
            rewind_attempt: None,
            edited_messages: 0,
            edit_cancel_focus: cx.focus_handle(),
            edit_confirm_focus: cx.focus_handle(),
            edit_keep_focus: cx.focus_handle(),
            queue_resume_focus: cx.focus_handle().tab_stop(true),
            queue_selected: None,
            queue_collapsed: false,
            queue_editor: None,
            queue_motion_rows: Vec::new(),
            bubble_entrances: Vec::new(),
            animating_bubbles: HashMap::new(),
            interrupt_requested_for_turn: None,
            stop_confirmation: None,
            stop_confirmation_timer: None,
            conversation_focus,
            images,
            previews,
            transcript_previews: rustc_hash::FxHashMap::default(),
            pending_image_pastes: 0,
            pending_attachment_picker: false,
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
            message_focus: rustc_hash::FxHashMap::default(),
            copied_message: None,
            copied_code_blocks: HashMap::new(),
            snippets: HashMap::new(),
            preview_snapshot: None,
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
            status_card_keys: HashSet::new(),
            task_cache: None,
            transcript_cache: None,
            minimap_view: None,
            snippet_sources: std::cell::RefCell::default(),
            turn_history: TurnHistory::default(),
            history_earlier_focus: cx.focus_handle().tab_stop(true).tab_index(0),
            history_latest_focus: cx.focus_handle().tab_stop(true).tab_index(0),
            pending_history_jump: None,
            dashboard_tray: None,
            dashboard_pending: false,
            dashboard_handoffs: Vec::new(),
            dashboard_status_expanded: false,
            dashboard_status_signature: Vec::new(),
            dashboard_more_focus: cx.focus_handle(),
            dashboard_tray_observer: None,
            dashboard_retry_focus: std::array::from_fn(|_| cx.focus_handle()),
            script_runtime: None,
            checkout_context: None,
            subscriptions,
        };
        view.subscriptions
            .push(cx.observe(&view.session, |view, _, cx| view.reconcile_snippets(cx)));
        view.subscriptions.push(cx.on_app_quit(|view, cx| {
            for snippet in view.snippets.values() {
                for run in &snippet.runs {
                    run.terminal.update(
                        cx,
                        crate::entities::terminal_session::TerminalSession::stop_script,
                    );
                }
            }
            async {}
        }));
        view.sync_native_plans(window, cx);
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
        let reserved: HashSet<_> = self
            .bubble_entrances
            .iter()
            .filter_map(|pending| state.submission_item_id(pending.submission))
            .map(str::to_owned)
            .collect();
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
                    || (!queued_steering && !state.submission_accepted(pending.submission)));
            if let Some(optimistic) = &mut pending.optimistic {
                optimistic["localSteering"] = steering_pending.into();
            }
            pending.existing.extend(consumed.iter().cloned());
            let owned_id = state.submission_item_id(pending.submission);
            let id = state.saved.items.iter().find_map(|item| {
                let id = item["id"].as_str()?;
                (item["type"] == "userMessage"
                    && !consumed.contains(id)
                    && (owned_id.map_or_else(
                        || {
                            !pending.existing.contains(id)
                                && !reserved.contains(id)
                                && codex::item_text(item) == pending.text
                                && pending.optimistic.as_ref().is_none_or(|optimistic| {
                                    let images = |message: &serde_json::Value| {
                                        message["content"]
                                            .as_array()
                                            .into_iter()
                                            .flatten()
                                            .filter_map(|part| {
                                                part["url"]
                                                    .as_str()
                                                    .or_else(|| part["image_url"].as_str())
                                            })
                                            .map(str::to_owned)
                                            .collect::<Vec<_>>()
                                    };
                                    images(item) == images(optimistic)
                                })
                        },
                        |owned_id| owned_id == id,
                    )))
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
                    pending.optimistic = Some(echo.into_value());
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
            optimistic: Some(serde_json::json!({"id":id, "type":"userMessage", "content":content, "localSteering":steering, "localQueuedSteering":queued_steering, "localAfterItem":state.saved.items.last().and_then(|item| item["id"].as_str()), "groveReceivedAt":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64})),
        });
        self.follow_after_submit = true;
    }
    fn draft_action_pending(&self, cx: &App) -> bool {
        let state = &self.session.read(cx).state;
        self.submitted.is_some()
            || self.pending_image_pastes > 0
            || self.pending_attachment_picker
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

    /// Collapse once per arriving identity, never on a routine rerender.
    fn sync_status_cards(&mut self, state: &codex::Snapshot) {
        let mut keys = HashSet::new();
        if state.session_status() == "Review" {
            if let Some(period) = state.review_completion() {
                keys.insert(format!("review:{period}"));
            }
        }
        if state.error.is_some() || state.status == codex::Status::Disconnected {
            keys.insert(format!("notice:{:?}:{:?}", state.status, state.error));
        }
        if state.plan_ready() {
            for plan in &state.saved.rich_plans.revisions {
                keys.insert(format!(
                    "plan:{}:{}",
                    plan.document.plan_id, plan.document.revision_id
                ));
            }
        }
        for approval in &state.approvals {
            keys.insert(Self::approval_key(approval, state.interaction_epoch));
        }
        for question in &state.questions {
            keys.insert(question_key(state, question));
        }
        for request in &state.elicitations {
            keys.insert(format!("elicitation:{request:?}"));
        }
        if keys.difference(&self.status_card_keys).next().is_some() {
            self.tasks_expanded = false;
        }
        self.status_card_keys = keys;
    }

    fn status_card_header(
        &self,
        key: &'static str,
        label: &'static str,
        kind: StatusKind,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let expanded = !self.expanded.contains(key);
        super::components::inline_disclosure(key, label)
            .debug_selector(move || key.into())
            .w_full()
            .h(rpx(TASK_STRIP_HEAD_H))
            .justify_start()
            .gap(rpx(SPACE_SM))
            .text_size(rpx(TEXT_SMALL))
            .aria_expanded(expanded)
            .child(crate::icons::icon(
                if expanded { "chev-down" } else { "chev-right" },
                ICON_MD,
                c::FG_DIM(),
            ))
            .child(status::label(key, kind, label, ICON_SM, cx))
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if action_key(event) {
                    if !this.expanded.remove(key) {
                        this.expanded.insert(key.into());
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.expanded.remove(key) {
                    this.expanded.insert(key.into());
                }
                cx.notify();
            }))
    }

    /// Current session feedback shares the task dock’s exact horizontal inset.
    fn session_review(
        &self,
        attached_above: bool,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if self.transcript_only() || state.session_status() != "Review" {
            return None;
        }
        let work_period = state.review_completion()?;
        let summary = self.transcript_cache.as_ref().map_or_else(
            || {
                super::review_summary::ReviewSummary::from_snapshot(state)
                    .map(|summary| review_overview(&summary.session_text()))
                    .filter(|summary| !summary.is_empty())
            },
            |cache| {
                cache
                    .review_overview
                    .get_or_init(|| {
                        super::review_summary::ReviewSummary::from_snapshot(state)
                            .map(|summary| review_overview(&summary.session_text()))
                            .filter(|summary| !summary.is_empty())
                    })
                    .clone()
            },
        );
        let expanded = summary.is_some() && !self.expanded.contains("session-review-disclosure");
        Some(
            composer_tray(window)
                .when(attached_above, gpui::Styled::rounded_none)
                // Keep intrinsic height; the containing rail removes its gap.
                .mb_0()
                .pb(rpx(SPACE_LG))
                .when(!expanded, |panel| panel.bg(c::BG()))
                .id("session-review")
                .debug_selector(|| "session-review".into())
                .role(gpui::Role::Group)
                .aria_label("Ready for review")
                .min_w_0()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_LG))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .when(summary.is_some(), |header| {
                                    header.child(self.status_card_header(
                                        "session-review-disclosure",
                                        "Ready for review",
                                        StatusKind::Success,
                                        cx,
                                    ))
                                })
                                .when(summary.is_none(), |header| {
                                    header.child(
                                        div()
                                            .h(rpx(TASK_STRIP_HEAD_H))
                                            .flex()
                                            .items_center()
                                            .child(status::label(
                                                "session-review-status",
                                                StatusKind::Success,
                                                "Ready for review",
                                                ICON_SM,
                                                cx,
                                            )),
                                    )
                                }),
                        )
                        .child(
                            form_action(
                                "session-mark-reviewed",
                                "Mark reviewed",
                                false,
                                window,
                                cx,
                            )
                            .icon(
                                gpui_component::Icon::default()
                                    .path("icons/check.svg")
                                    .size(rpx(ICON_SM)),
                            )
                            .debug_selector(|| "session-mark-reviewed".into())
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    let current = &this.session.read(cx).state;
                                    if current.session_status() == "Review"
                                        && current.review_completion() == Some(work_period)
                                    {
                                        this.session.update(cx, |session, cx| {
                                            session.send(Action::MarkReviewed { work_period }, cx);
                                        });
                                        this.input.focus_handle(cx).focus(window, cx);
                                    }
                                },
                            )),
                        ),
                )
                .when(expanded, |panel| {
                    panel.child(
                        div()
                            .id("session-review-summary")
                            .debug_selector(|| "session-review-summary".into())
                            .line_clamp(2)
                            .pl(rpx(TASK_MARK_INSET))
                            .pb(rpx(SPACE_LG))
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(summary.unwrap_or_default()),
                    )
                })
                .into_any_element(),
        )
    }

    fn session_notice(
        &self,
        attached_above: bool,
        state: &codex::Snapshot,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        // Catalog failures already have an actionable home in the Dashboard slot.
        if self.dashboard_tray.is_some()
            && self.session.read(cx).catalog_error.is_some()
            && self.session.read(cx).catalog_error == state.error
        {
            return None;
        }
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
        let lower = body.to_ascii_lowercase();
        if [
            "sign in",
            "login",
            "log in",
            "not logged",
            "authentication",
            "unauthorized",
            "oauth",
        ]
        .iter()
        .any(|word| lower.contains(word))
        {
            let provider = state.saved.provider;
            let cwd = state.saved.cwd.clone();
            let args = state.saved.config_args.clone();
            actions = actions.child(
                form_action("session-connections", "Manage sign-in", false, window, cx)
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/external-link.svg")
                            .size(rpx(ICON_SM)),
                    )
                    .on_click(cx.listener(move |_, _, window, cx| {
                        cx.emit(CodexViewEvent::OpenConnections(
                            provider,
                            cwd.clone(),
                            args.clone(),
                            window.focused(cx),
                        ));
                    })),
            );
        }
        if disconnected {
            actions = actions.child(
                form_action("codex-reconnect", "Reconnect", false, window, cx)
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/refresh.svg")
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
            composer_tray(window)
                .when(attached_above, gpui::Styled::rounded_none)
                // Negative tray overlap can erase the rail's intrinsic height.
                .mb_0()
                .pb(rpx(SPACE_LG))
                .id("codex-error")
                .debug_selector(|| "codex-error".into())
                .role(gpui::Role::Alert)
                .flex_shrink_0()
                .flex()
                .flex_col()
                .child(self.status_card_header("session-notice-disclosure", title, notice_kind, cx))
                .when(
                    !self.expanded.contains("session-notice-disclosure"),
                    |panel| {
                        panel.child(
                            div()
                                .pl(rpx(TASK_LABEL_INSET))
                                .pb(rpx(SPACE_LG))
                                .child(content),
                        )
                    },
                )
                .into_any_element(),
        )
    }

    /// The docked, read-only checklist; jumps identify the task’s transcript start.
    fn task_strip(
        &self,
        attached_above: bool,
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
            .child(
                div()
                    .debug_selector(|| "codex-task-header-label".into())
                    .flex_shrink_0()
                    .text_color(if expanded { c::FG() } else { c::FG_DIM() })
                    .group_hover("neutral-control", |text| text.text_color(c::FG()))
                    .child("Tasks"),
            )
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
            .group("neutral-control")
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
            .when(expanded, |style| style.text_color(c::FG()))
            .hover(|style| style.text_color(c::FG()))
            .focus_visible(|style| style.bg(c::MENU_HOVER()))
            .child(crate::icons::icon(
                if expanded { "chev-down" } else { "chev-right" },
                ICON_MD,
                if expanded { c::FG() } else { c::FG_DIM() },
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
                            .pl(rpx(TASK_LABEL_INSET))
                            .pr(rpx(SPACE_LG))
                            .py(rpx(SPACE_SM))
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .whitespace_normal()
                            .child(
                                div()
                                    .debug_selector(|| "codex-task-explanation-text".into())
                                    .child(explanation),
                            ),
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
                        .pl(rpx(TASK_MARK_INSET))
                        .pr(rpx(SPACE_LG))
                        .rounded(rpx(RADIUS_CONTROL))
                        .text_size(rpx(TEXT_SMALL))
                        .hover(|style| style.bg(c::BG_HOVER()).text_color(c::FG()))
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
                                    this.jump_to_transcript_item(child, window, cx);
                                    cx.stop_propagation();
                                }
                            },
                        ))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.jump_to_transcript_item(child, window, cx);
                        })),
                );
            }
            rows
        });
        composer_tray(window)
            .when(attached_above, gpui::Styled::rounded_none)
            .id("codex-task-strip-panel")
            .debug_selector(|| "codex-task-strip-panel".into())
            // Balance the visible bottom attachment against the header's top inset.
            .when(!expanded, |panel| panel.pb(rpx(SPACE_2XL + SPACE_LG)))
            .when(!expanded, |panel| panel.bg(c::BG()))
            .role(gpui::Role::Group)
            .aria_label(format!("Agent tasks, {progress}"))
            // Shrinks by default: in a short pane the strip yields height to
            // the composer, its list scrolls, and the header row stays visible.
            .min_h(rpx(TASK_STRIP_HEAD_H))
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(header)
            .children(body)
            .into_any_element()
    }

    fn render_provider_notices(
        &mut self,
        index: usize,
        end: usize,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let notices = &state.saved.items[index..end];
        let key = format!(
            "provider-notices:{}:{}",
            state.saved.thread_id.as_deref().unwrap_or_default(),
            notices[0]["id"]
                .as_str()
                .map_or_else(|| index.to_string(), str::to_owned)
        );
        let expanded = self.expanded.contains(&key);
        let mcp = notices.iter().all(|notice| {
            notice["title"].as_str().is_some_and(|title| {
                title.starts_with("MCP server ") && title.ends_with(" is unavailable")
            })
        });
        let mut label = if mcp {
            format!(
                "{} MCP server{} unavailable",
                notices.len(),
                if notices.len() == 1 { "" } else { "s" }
            )
        } else if notices.len() == 1 {
            notices[0]["title"]
                .as_str()
                .unwrap_or("Provider warning")
                .to_owned()
        } else {
            format!("{} provider warnings", notices.len())
        };
        if mcp
            && notices.iter().all(|notice| {
                notice["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("Authenticate this MCP server"))
            })
        {
            label.push_str(" · Authentication required");
        }
        let focus = self
            .ensure_message_focus(&key, window, cx)
            .tab_stop(true)
            .tab_index(0);
        let keyboard_key = key.clone();
        let mut group = div()
            .id(("provider-notice", index))
            .debug_selector(move || format!("provider-notice-{index}"))
            .w_full()
            .max_w(rpx(CHAT_MAX_W))
            .min_w_0()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_SM))
            .child(
                super::components::inline_disclosure("provider-notice-toggle", label.clone())
                    .id(("provider-notice-toggle", index))
                    .debug_selector(move || format!("provider-notice-toggle-{index}"))
                    .track_focus(&focus)
                    .tab_index(0)
                    .aria_expanded(expanded)
                    .w_full()
                    .h_auto()
                    .min_h(rpx(CHROME_CONTROL_H))
                    .justify_start()
                    .gap(rpx(SPACE_SM))
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .focus_visible(|style| style.bg(c::BG_HOVER()))
                    .child(crate::icons::icon(
                        if expanded { "chev-down" } else { "chev-right" },
                        ICON_SM,
                        c::FG_DIM(),
                    ))
                    .child(crate::icons::icon("message-warning", ICON_MD, c::AMBER()))
                    .child(div().flex_1().min_w_0().child(label))
                    .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                        if action_key(event) {
                            this.toggle_expansion(&keyboard_key, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        focus.focus(window, cx);
                        this.toggle_expansion(&key, cx);
                    })),
            );
        // Expansion is immediate input feedback, matching the transcript tool disclosures.
        // Keep literal details intact and show identical recovery text once per group.
        if expanded {
            let mut details = div()
                .w_full()
                .min_w_0()
                .pl(rpx(ICON_MD + SPACE_LG))
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG));
            let shared_text = notices
                .iter()
                .all(|notice| notice["text"] == notices[0]["text"]);
            for (offset, notice) in notices.iter().enumerate() {
                let item_index = index + offset;
                details = details.child(
                    div().w_full().min_w_0().text_size(rpx(TEXT_SMALL)).child(
                        notice["title"]
                            .as_str()
                            .unwrap_or("Provider warning")
                            .to_owned(),
                    ),
                );
                if !shared_text || offset + 1 == notices.len() {
                    details = details.child(literal_notice_text(
                        gpui::SharedString::from(format!("provider-notice-text-{item_index}")),
                        notice["text"].as_str().unwrap_or_default(),
                    ));
                }
            }
            group = group.child(details);
        }
        if mcp {
            let provider = state.saved.provider;
            let cwd = state.saved.cwd.clone();
            let args = state.saved.config_args.clone();
            group = group.child(
                super::components::form_action(
                    ("provider-connections", index),
                    "Manage connection",
                    false,
                    window,
                    cx,
                )
                .icon(
                    gpui_component::Icon::default()
                        .path("icons/external-link.svg")
                        .size(rpx(ICON_SM)),
                )
                .debug_selector(move || format!("provider-connections-{index}"))
                .on_click(cx.listener(move |_, _, window, cx| {
                    cx.emit(CodexViewEvent::OpenConnections(
                        provider,
                        cwd.clone(),
                        args.clone(),
                        window.focused(cx),
                    ));
                })),
            );
        }
        group.into_any_element()
    }

    fn render_message_minimap(
        &mut self,
        prepared: Rc<PreparedTranscript>,
        anchors: &[(usize, usize)],
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        #[cfg(test)]
        let _phase = measured_slot::phases::Section::minimap();
        if prepared.minimap_entries.is_empty() {
            self.minimap_view = None;
            return div().into_any_element();
        }
        let near_bottom = self.scroll.max_offset().y + self.scroll.offset().y
            <= rpx(SPACE_20).to_pixels(window.rem_size());
        let active = if near_bottom {
            anchors.last()
        } else {
            let boundary = anchors.partition_point(|(_, child)| *child <= self.scroll.top_item());
            anchors.get(boundary.saturating_sub(1))
        }
        .map(|(index, _)| *index);
        let preview_width = (self.scroll.bounds().size.width
            - rpx(SPACE_3XL).to_pixels(window.rem_size()))
        .max(rpx(CHROME_CONTROL_H).to_pixels(window.rem_size()))
        .min(rpx(MINIMAP_PREVIEW_W - SPACE_3XL).to_pixels(window.rem_size()));
        // Accessibility and inspector traversal require the ordinary eager tree.
        if window.is_a11y_active() || window.is_inspector_picking(cx) {
            return self.build_message_minimap(
                &prepared.minimap_entries,
                active,
                preview_width,
                window,
                cx,
            );
        }
        let colors = [
            c::FG(),
            c::FG_DIM(),
            c::BG_HOVER(),
            c::SURFACE_RAISED(),
            c::BORDER(),
            c::BG(),
        ];
        let parent = cx.entity().downgrade();
        let minimap = self
            .minimap_view
            .get_or_insert_with(|| {
                cx.new(|_| TranscriptMinimap {
                    parent,
                    prepared: prepared.clone(),
                    active,
                    preview_width,
                    colors,
                })
            })
            .clone();
        minimap.update(cx, |view, cx| {
            if !Rc::ptr_eq(&view.prepared, &prepared)
                || view.active != active
                || view.preview_width != preview_width
                || view.colors != colors
            {
                view.prepared = prepared;
                view.active = active;
                view.preview_width = preview_width;
                view.colors = colors;
                cx.notify();
            }
        });
        let mut style = gpui::StyleRefinement::default();
        style.size.width = Some(rpx(CHROME_CONTROL_H).into());
        style.size.height = Some(gpui::relative(1.).into());
        style.flex_shrink = Some(0.);
        minimap.cached(style).into_any_element()
    }

    fn build_message_minimap(
        &self,
        entries: &[MessageMinimapEntry],
        active: Option<usize>,
        preview_width: gpui::Pixels,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        #[cfg(test)]
        MINIMAP_BUILDS.with(|count| count.set(count.get() + 1));
        let rail = div()
            .id("codex-message-minimap")
            .debug_selector(|| "codex-message-minimap".into())
            .role(gpui::Role::Group)
            .aria_label("Conversation messages")
            .h_full()
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
        let marker_count = entries.len() as f32;
        let mut stack = div()
            .w_full()
            .flex()
            .flex_col()
            .my_auto()
            .h(rpx(marker_count * SPACE_LG))
            .min_h(rpx(marker_count * SPACE_SM))
            .flex_shrink_1();
        for entry in entries {
            let index = entry.item_index;
            let selected = active == Some(index);
            let prompt = entry.prompt_preview.clone();
            let response = entry.response_preview.clone();
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
                .aria_label(entry.label.clone())
                .aria_description(entry.description.clone())
                .when(selected, |style| {
                    style.bg(c::BG_HOVER()).text_color(c::FG())
                })
                .focus_visible(|style| style.bg(c::BG_HOVER()))
                .child(
                    div()
                        .w(rpx(if selected { SPACE_3XL } else { SPACE_LG }))
                        .h(rpx(SPACE_XS))
                        .bg(if selected { c::FG() } else { c::FG_DIM() })
                        .group_hover("neutral-control", |style| style.bg(c::FG())),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.jump_to_transcript_item(index, window, cx);
                }))
                .on_key_down(cx.listener(
                    move |this, event: &gpui::KeyDownEvent, window, cx| {
                        if action_key(event) {
                            this.jump_to_transcript_item(index, window, cx);
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
        if self.transcript_only() {
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
}
impl Focusable for CodexView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}
impl CodexView {
    fn next_reply_mode_eligible(&self, state: &codex::Snapshot) -> bool {
        !self.transcript_only()
            && !self.dashboard_pending
            && self.dashboard_tray.is_none()
            && self.plan_answer.is_none()
            && self.queue_editor.is_none()
            && self.submitted.is_none()
            && !self.edit_request_pending
            && self.edit_confirmation.is_none()
            && state.saved.pending_edit.is_none()
            && state.questions.is_empty()
            && state.approvals.is_empty()
            && state.elicitations.is_empty()
            && state.queued_prompts.is_empty()
            && !state.status.busy()
            && self.images.is_empty()
            && self.reference.is_none()
            && self.pending_image_pastes == 0
            && !self.pending_attachment_picker
            && self.description_scope.is_none()
            && self.review_scope.is_none()
    }

    fn dismiss_next_reply(&mut self, cx: &mut Context<Self>) {
        self.next_reply.dismiss();
        self.next_reply_request = None;
        self.next_reply_control = None;
        self.next_reply_task = None;
        self.input.update(cx, |input, cx| {
            input.set_reply_preview(None, c::FG_DIM(), cx);
        });
        cx.notify();
    }

    fn next_reply_text(&self, window: &Window, cx: &App) -> Option<String> {
        let state = &self.session.read(cx).state;
        let input = self.input.read(cx);
        if self.next_reply_request.as_deref()
            != grove_core::next_reply::Request::capture(state)
                .as_ref()
                .map(|r| r.key.as_str())
            || !self.next_reply_mode_eligible(state)
            || self.menu.is_some()
            || self.popup.is_some()
            || self.composer_collapsed
            || !(input.focus_handle(cx).is_focused(window)
                || self
                    .next_reply_focus
                    .iter()
                    .any(|focus| focus.is_focused(window)))
            || !input.reply_preview_eligible()
            || self.suggestion_choices(state, cx).is_some()
        {
            return None;
        }
        self.next_reply.visible(&input.value()).map(str::to_owned)
    }

    fn accept_next_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(text) = self.next_reply_text(window, cx) else {
            return false;
        };
        if !self.input.focus_handle(cx).is_focused(window) {
            return false;
        }
        self.input.update(cx, |input, cx| {
            input.set_reply_preview(None, c::FG_DIM(), cx);
            input.accept_reply_preview(text.into(), window, cx);
        });
        // Explicit replacements use silent input events; persist the actual accepted draft.
        let draft = self.input.read(cx).value().to_string();
        self.session
            .update(cx, |session, cx| session.send(Action::Draft(draft), cx));
        true
    }

    fn move_next_reply(
        &mut self,
        previous: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.next_reply_text(window, cx).is_none() {
            return false;
        }
        self.next_reply.cycle(previous);
        self.input.focus_handle(cx).focus(window, cx);
        cx.notify();
        true
    }

    fn next_reply_navigation(&self, previous: bool, cx: &Context<Self>) -> gpui::AnyElement {
        let (id, label, glyph, index) = if previous {
            (
                "next-reply-previous",
                "Previous suggested reply",
                "chev-left",
                0,
            )
        } else {
            ("next-reply-next", "Next suggested reply", "chev-right", 1)
        };
        if self
            .next_reply
            .position()
            .is_none_or(|(_, count)| count < 2)
        {
            return super::components::disabled_header_control(id, label)
                .debug_selector(move || id.into())
                .child(crate::icons::icon(glyph, ICON_SM, c::FG_DIM()))
                .into_any_element();
        }
        header_control(id, label)
            .debug_selector(move || id.into())
            .track_focus(&self.next_reply_focus[index])
            .tab_index(0)
            .focus_visible(|style| style.bg(c::BG_HOVER()).text_color(c::FG()))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.move_next_reply(previous, window, cx);
            }))
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space")
                        && !event.keystroke.modifiers.modified()
                        && !event.is_held
                        && this.move_next_reply(previous, window, cx)
                    {
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                }),
            )
            .child(crate::icons::icon(glyph, ICON_SM, c::FG_DIM()))
            .into_any_element()
    }

    fn sync_next_reply(
        &mut self,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let request = grove_core::next_reply::Request::capture(state);
        if self
            .next_reply_request
            .as_ref()
            .is_some_and(|key| request.as_ref().is_none_or(|r| &r.key != key))
            || (self.next_reply_request.is_some() && !self.next_reply_mode_eligible(state))
        {
            self.dismiss_next_reply(cx);
        }
        if let Some(request) = request.filter(|r| self.next_reply_seen.as_ref() != Some(&r.key)) {
            self.next_reply_seen = Some(request.key.clone());
            if self.next_reply_mode_eligible(state) {
                self.next_reply = grove_core::next_reply::Candidate::pending();
                self.next_reply_request = Some(request.key.clone());
                let control = grove_core::next_reply::JobControl::default();
                let cancel = control.0.clone();
                self.next_reply_control = Some(control);
                let generate = self.next_reply_generate;
                self.next_reply_task = Some(cx.spawn(async move |this, cx| {
                    let key = request.key.clone();
                    let result = cx
                        .background_executor()
                        .spawn(async move { generate(&request, &cancel) })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if this.next_reply_request.as_ref() == Some(&key) {
                            this.next_reply_control = None;
                            let result = match result {
                                Ok(text) => text,
                                Err(error) => {
                                    tracing::debug!(%error, "Next reply generation unavailable");
                                    Vec::new()
                                }
                            };
                            this.next_reply.resolve(result);
                            cx.notify();
                        }
                    });
                }));
            }
        }
        let preview = self
            .next_reply_text(window, cx)
            .map(gpui::SharedString::from);
        self.input.update(cx, |input, cx| {
            input.set_reply_preview(preview, c::FG_DIM(), cx);
        });
    }
}

impl Render for CodexView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        let _phase = measured_slot::phases::Frame::start();
        #[cfg(test)]
        measured_slot::RENDERED_FOOTERS.with(|keys| keys.borrow_mut().clear());
        #[cfg(test)]
        measured_slot::reset_tool_detail_counts();
        if !self.selection_root_observed {
            if let Some(root) = window.root::<gpui_component::Root>().flatten() {
                self.subscriptions
                    .push(cx.observe(&root, |_, _, cx| cx.notify()));
                self.selection_root_observed = true;
            }
        }
        self.validate_stop_confirmation(cx);
        self.flush_dashboard_submission(window, cx);
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
            let source = self.session.read(cx).state.shared();
            if self
                .preview_snapshot
                .as_ref()
                .is_none_or(|(old, id, _)| !Rc::ptr_eq(old, &source) || id != child)
            {
                let projected = Rc::new(source.subagent_snapshot(child));
                self.preview_snapshot = Some((source.clone(), child.clone(), projected));
            }
            self.preview_snapshot.as_ref().map_or_else(
                || Rc::new(source.subagent_snapshot(child)),
                |(_, _, projected)| projected.clone(),
            )
        } else {
            self.session.read(cx).state.shared()
        };
        if self.edit_confirmation.as_ref().is_some_and(|target| {
            target.thread_id != state.saved.thread_id
                || state
                    .saved
                    .items
                    .get(target.index)
                    .is_none_or(|item| item["id"].as_str() != Some(&target.message_id))
        }) {
            self.edit_confirmation = None;
        }
        if self
            .composer_menu_scope
            .as_ref()
            .is_some_and(|scope| *scope != composer_scope(&state))
        {
            self.close_menu(cx);
        }
        self.sync_native_plans(window, cx);
        self.sync_plan_composer_pending(cx);

        if state.edited_messages != self.edited_messages {
            self.edited_messages = state.edited_messages;
            self.edit_request_pending = false;
            self.rewind_attempt = None;
            self.edit_confirmation = None;
            self.load_restored_composer(&state, window, cx);
            self.bubble_entrances.clear();
            self.animating_bubbles.clear();
            self.stream_text.clear();
            self.set_composer_collapsed(false, cx);
        }

        if self
            .rewind_attempt
            .is_some_and(|attempt| state.rewind_attempts > attempt)
            && (state.error.is_some() || state.saved.pending_edit.is_none())
        {
            self.edit_request_pending = false;
            self.rewind_attempt = None;
        }
        // checkout_context contains chrome labels such as “Main checkout”, not
        // filesystem paths. Resolve document links against the saved worktree.
        let html_cwd = std::path::PathBuf::from(&state.saved.cwd);
        self.file_browser
            .update(cx, |browser, cx| browser.set_root(html_cwd.clone(), cx));
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
            self.selection_subscriptions.clear();
            self.message_focus.clear();
            self.focus_subscriptions.clear();
            self.focus_sources.clear();
            self.focus_revision = None;
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
                draft.activity_anchor.focus.is_focused(window)
                    || draft.input.focus_handle(cx).is_focused(window)
                    || draft.feedback.focus_handle(cx).is_focused(window)
            }) || self.elicitation_drafts.values().any(|draft| {
                draft.activity_anchor.focus.is_focused(window)
                    || draft.fields.iter().any(|field| {
                        field.input.focus_handle(cx).is_focused(window)
                            || field
                                .choice_focus
                                .values()
                                .any(|focus| focus.is_focused(window))
                    })
            }) || self
                .question_drafts
                .values()
                .any(|draft| draft.activity_anchor.focus.is_focused(window));
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
            .is_some_and(|target| state.submission_accepted(target))
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
        // Submission receipts and restored/queued drafts must settle before
        // deciding whether the newly completed response has an empty composer.
        self.sync_next_reply(&state, window, cx);
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
                let cache = Rc::new(TranscriptRenderCache::refreshed(
                    state,
                    optimistic_items,
                    self.transcript_cache.as_deref(),
                ));
                self.transcript_previews.retain(|(index, _), (source, _)| {
                    cache
                        .state
                        .saved
                        .items
                        .get(*index)
                        .is_some_and(|item| std::ptr::eq(source.as_ref(), item.as_ref()))
                });
                self.transcript_cache = Some(cache.clone());
                cache
            }
        };
        self.prune_transcript_controls(&transcript);
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
            if self.transcript_only() {
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
            self.turn_history = TurnHistory::default();
            self.pending_history_jump = None;
            self.message_slots.clear();
            self.work_header_slots.clear();
            self.tool_slots.clear();
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
        self.stream_text.retain(|key, _| {
            transcript.id_index.contains_key(key)
                || key
                    .strip_prefix("message-")
                    .and_then(|index| index.parse::<usize>().ok())
                    .and_then(|index| state.saved.items.get(index))
                    .is_some_and(|item| !item["id"].is_string())
        });
        if self.follow_after_submit {
            self.last_follow_offset = None;
        }
        let now = cx.background_executor().now();
        let elapsed = self
            .motion_frame
            .replace(now)
            .map_or(0.0, |previous| now.duration_since(previous).as_secs_f32());
        if !self.transcript_only()
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
            self.message_slots.clear();
            self.work_header_slots.clear();
            self.tool_slots.clear();
            return div()
                .id("codex-conversation")
                .track_focus(&self.conversation_focus)
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
        let history_start = {
            let following = near_bottom
                && !self.composer_collapsed
                && self.edit_confirmation.is_none()
                && !gpui_component::text::has_active_text_selection(window, cx)
                && window.text_selection_snapshot(cx).is_none();
            self.turn_history
                .sync(&state.saved.items, &transcript.minimap_entries, following)
        };
        let history_start =
            self.reveal_activity_request(history_start, &state, &transcript, window, cx);
        let response_groups = &transcript.response_groups;
        let response_ends = &transcript.response_ends;
        let minimap_entries = &transcript.minimap_entries;
        let mut minimap_anchors = Vec::new();
        // The first rendered child at or after each item, for jump-to-start.
        let mut item_children = vec![0; state.saved.items.len() - history_start];
        let mut message_child = 0;
        let mut rendered_selection_keys = HashSet::new();
        let mut rendered_slot_keys = HashSet::new();
        let mut rendered_header_keys = HashSet::new();
        let mut rendered_tool_keys = HashSet::new();
        let message_frame = Rc::new(measured_slot::MessageFrame {
            rich_messages: transcript.rich_messages.clone(),
            review_display: transcript.review_display.clone(),
            state: state.clone(),
            prepared: transcript.prepared.clone(),
            initial_transcript,
            near_bottom,
            elapsed,
        });
        // Use the allocated pane, rather than the entire app window (split/grid).
        let pane_width = f32::from(self.transcript_bounds.get().size.width)
            / (f32::from(window.rem_size()) / crate::zoom::REM_BASE);
        let content_inset = if pane_width > 0. && pane_width < MODAL_W_LG {
            SPACE_LG
        } else {
            SPACE_20
        };
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
            .px(rpx(content_inset))
            .py(rpx(SPACE_3XL))
            .gap(rpx(SPACE_20));
        if history_start > 0 || self.turn_history.expanded {
            let hidden = history::history_boundaries(&state.saved.items, minimap_entries)
                .partition_point(|index| *index < history_start);
            messages = messages.child(self.render_history_disclosure(hidden, cx));
            message_child += 1;
        }
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
            .map(AsRef::as_ref)
            .chain(std::iter::once(&serde_json::Value::Null))
            .enumerate()
            .skip(history_start)
        {
            // Flush before inspecting the next source item: the previous final item
            // may be hidden commentary, a checklist, or a collapsed tool.
            if let Some(group_index) = response_ends[index].filter(|_| index > history_start) {
                let group = &response_groups[group_index];
                let footer = if window.is_a11y_active()
                    || measured_slot::force_eager()
                    || measured_slot::force_footer_eager()
                {
                    self.response_footer(group, &state, true, window, cx)
                } else {
                    let focus = self.ensure_message_focus(&group.key, window, cx);
                    let copy_focus =
                        self.ensure_control_focus(&format!("{}:copy", group.key), window, cx);
                    measured_slot::MeasuredResponseFooter::new(
                        cx.entity().downgrade(),
                        message_frame.clone(),
                        group_index,
                        focus,
                        copy_focus,
                    )
                    .into_any_element()
                };
                messages = messages.child(footer);
                message_child += 1;
            }
            if index < state.saved.items.len() {
                item_children[index - history_start] = message_child;
                let blocks = self.question_blocks_at(index, &state, window, cx);
                message_child += blocks.len();
                messages = messages.children(blocks);
            }
            // Group only adjacent notices; never cross a question or another message.
            // Skip before work headers so grouped diagnostics do not leave empty work rows.
            if index > history_start
                && index < state.saved.items.len()
                && item["type"] == "providerNotice"
                && state.saved.items[index - 1]["type"] == "providerNotice"
                && !self.has_question_anchor(index)
            {
                continue;
            }
            if let Some((key, label, active)) =
                transcript.work.header(&state, &transcript.prepared, index)
            {
                message_child += 1;
                let expanded = self.expanded.contains(&key);
                let focus_key = format!(
                    "work-header:{}:{index}:{key}",
                    state.saved.thread_id.as_deref().unwrap_or_default()
                );
                // The original button used an implicit tabbable handle. An
                // explicit tracked handle must preserve that metadata itself.
                let focus = self
                    .ensure_message_focus(&focus_key, window, cx)
                    .tab_stop(true)
                    .tab_index(0);
                self.message_focus.insert(focus_key.clone(), focus.clone());
                let header = if !active
                    && !expanded
                    && !window.is_a11y_active()
                    && !measured_slot::force_eager()
                    && !measured_slot::force_header_eager()
                {
                    rendered_header_keys.insert(focus_key.clone());
                    let cache = self.work_header_slots.entry(focus_key).or_default().clone();
                    measured_slot::MeasuredWorkHeader::new(
                        cx.entity().downgrade(),
                        index,
                        key.clone(),
                        label.clone(),
                        focus,
                        cache,
                    )
                    .into_any_element()
                } else {
                    self.render_work_header(index, &key, &label, expanded, &focus, cx)
                };
                messages = messages.child(header);
                if expanded && !transcript.work.parent_activity[index] {
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
            if transcript.commentary[index]
                && !self
                    .expanded
                    .contains(WorkIndex::key(&transcript.prepared, index))
                && !transcript.work.active[index]
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
                let end = (index + 1..state.saved.items.len())
                    .find(|next| {
                        state.saved.items[*next]["type"] != "providerNotice"
                            || self.has_question_anchor(*next)
                    })
                    .unwrap_or(state.saved.items.len());
                message_child += 1;
                messages =
                    messages.child(self.render_provider_notices(index, end, &state, window, cx));
                continue;
            }
            if tasks::is_checklist(item) || grove_core::activity::is_internal(item) {
                // Checklist updates belong in the docked panel, not the transcript.
                continue;
            }
            let tool = is_tool_item(item);
            let has_visual = |index: usize| {
                state.saved.items.get(index).is_some_and(|item| {
                    item["id"]
                        .as_str()
                        .is_some_and(|id| transcript.visual_items.contains(id))
                })
            };
            if tool && has_visual(index) {
                let focus = self.ensure_control_focus(&format!("visual-tool-{index}"), window, cx);
                let mut publication = div()
                    .id(("codex-visual-publication", index))
                    .debug_selector(move || format!("codex-visual-publication-{index}"))
                    .w_full()
                    .max_w(rpx(CHAT_MAX_W))
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_LG))
                    .child(self.render_tool_row(index, &state, false, &focus, window, cx));
                for reference in state
                    .saved
                    .visuals
                    .iter()
                    .filter(|r| item["id"] == r.item_id)
                {
                    publication = publication.child(
                        self.html
                            .update(cx, |html, cx| html.published(reference, window, cx)),
                    );
                }
                messages = messages.child(publication);
                message_child += 1;
                continue;
            }
            if tool {
                if index > 0
                    && is_tool_item(&state.saved.items[index - 1])
                    && !has_visual(index - 1)
                    && (state.saved.items[index - 1]["type"] == "reasoning")
                        == (kind == "reasoning")
                    && !self.has_question_anchor(index)
                {
                    continue;
                }
                let end = state.saved.items[index..]
                    .iter()
                    .position(|item| {
                        item["id"]
                            .as_str()
                            .is_some_and(|id| transcript.visual_items.contains(id))
                            || !is_tool_item(item)
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
                        self.expanded
                            .contains(WorkIndex::key(&transcript.prepared, item_index))
                            || (transcript.work.tool_visible(&state, item_index)
                                && transcript.work.active[item_index])
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
                        state
                            .saved
                            .items
                            .get(end)
                            .is_some_and(|item| is_tool_item(item))
                            && transcript.work.tool_visible(&state, end)
                            && (transcript.work.active[end]
                                || self
                                    .expanded
                                    .contains(WorkIndex::key(&transcript.prepared, end))),
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
                        super::components::inline_disclosure(
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
                        .when(expanded, |style| style.text_color(c::FG()))
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
                        #[cfg(test)]
                        measured_slot::observe_tool_detail(
                            detail_index >= transcript.work.active_start,
                        );
                        let tool_key = tool_expansion_key(tool, detail_index);
                        let focus_key = format!(
                            "tool-row:{}:{detail_index}:{tool_key}",
                            state.saved.thread_id.as_deref().unwrap_or_default()
                        );
                        let focus = self.ensure_control_focus(&focus_key, window, cx);
                        let shimmer = transcript.work.active[detail_index]
                            && transcript.work.tool_visible(&state, detail_index);
                        let detail_expanded = self.expanded.contains(&tool_key);
                        let eager = tool_is_running(&state, detail_index)
                            || shimmer
                            || window.is_a11y_active()
                            || gpui_component::text::has_active_text_selection(window, cx)
                            || window.is_inspector_picking(cx)
                            || measured_tool::force_eager();
                        let row = if eager {
                            self.render_tool_row(detail_index, &state, shimmer, &focus, window, cx)
                        } else {
                            rendered_tool_keys.insert(focus_key.clone());
                            let cache = self.tool_slots.entry(focus_key).or_default().clone();
                            measured_tool::MeasuredTool::new(
                                cx.entity().downgrade(),
                                state.clone(),
                                detail_index,
                                tool_key,
                                focus,
                                detail_expanded,
                                cache,
                            )
                            .into_any_element()
                        };
                        group = group.child(row);
                    }
                }
                message_child += 1;
                messages = messages.child(group);
                continue;
            }
            let message_id = item["id"]
                .as_str()
                .map_or_else(|| format!("message-{index}"), str::to_owned);
            let owner_key = format!(
                "{}:{}",
                state.saved.thread_id.as_deref().unwrap_or(""),
                message_id
            );
            let receipt = kind == "userMessage"
                && state
                    .saved
                    .rich_plans
                    .approval_for_payload(&codex::item_text(item))
                    .is_some();
            if !tool && !receipt {
                rendered_selection_keys.insert(owner_key.clone());
            }
            let slot_key = format!("{owner_key}:{index}");
            // Streaming and entrance animations update follow state during Render;
            // keep those effects before the parent's scroll-follow decisions.
            let tail_stream = state.status.busy() && index + 1 == state.saved.items.len();
            let entrance = self
                .animating_bubbles
                .contains_key(item["id"].as_str().unwrap_or(""))
                || self.bubble_entrances.iter().any(|pending| {
                    pending
                        .optimistic
                        .as_ref()
                        .is_some_and(|bubble| bubble["id"] == item["id"])
                });
            let row = if matches!(kind, "userMessage" | "agentMessage")
                && !receipt
                && !tail_stream
                && !entrance
                && !window.is_a11y_active()
            {
                rendered_slot_keys.insert(slot_key.clone());
                let cache = self.message_slots.entry(slot_key).or_default().clone();
                measured_slot::MeasuredTranscriptSlot::new(
                    cx.entity().downgrade(),
                    index,
                    message_frame.clone(),
                    owner_key,
                    cache,
                )
                .into_any_element()
            } else {
                self.render_message_row(index, &message_frame, window, cx)
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
        if let Some(index) = self.pending_history_jump.take() {
            let child = item_children
                .get(index.saturating_sub(history_start))
                .copied()
                .unwrap_or(message_child.saturating_sub(1));
            self.jump_to_message(child, window, cx);
        }
        self.prune_idle_snippets(&rendered_selection_keys);
        if self.transcript_only() {
            self.message_slots
                .retain(|key, _| rendered_slot_keys.contains(key));
            self.work_header_slots
                .retain(|key, _| rendered_header_keys.contains(key));
            self.tool_slots
                .retain(|key, _| rendered_tool_keys.contains(key));
            self.selection_views
                .retain(|key, _| rendered_selection_keys.contains(key));
            self.selection_subscriptions
                .retain(|key, _| self.selection_views.contains_key(key));
            // Stay pinned through layout growth until a wheel gesture starts browsing.
            // Returning to the bottom resumes following without affecting the composer.
            if self.preview_follow_latest {
                self.scroll.scroll_to_bottom();
            }
            return div()
                .debug_selector(|| "codex-readonly-transcript".into())
                .track_focus(&self.input.focus_handle(cx))
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
        messages =
            messages.children(self.question_blocks_at(state.saved.items.len(), &state, window, cx));
        // Question edits and composer reflow can change the scroll range without
        // changing the transcript items. Pin against the new layout so the
        // footer or receipt stays reachable when the reader was at the bottom.
        if (!state.questions.is_empty() || self.has_question_anchor(state.saved.items.len()))
            && !self.composer_collapsed
            && self.edit_confirmation.is_none()
            && (self.composer_pin_bottom
                || self.scroll.max_offset().y + self.scroll.offset().y <= gpui::px(0.5))
        {
            self.scroll.scroll_to_bottom();
        }
        // Hidden, replaced and removed message owners must be dropped so the
        // native selection controller cannot copy their last painted text.
        self.selection_views
            .retain(|key, _| rendered_selection_keys.contains(key));
        self.selection_subscriptions
            .retain(|key, _| self.selection_views.contains_key(key));
        self.message_slots
            .retain(|key, _| rendered_slot_keys.contains(key));
        self.tool_slots
            .retain(|key, _| rendered_tool_keys.contains(key));
        self.work_header_slots
            .retain(|key, _| rendered_header_keys.contains(key));
        let minimap =
            self.render_message_minimap(transcript.prepared.clone(), &minimap_anchors, window, cx);
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
                    let selection = window.text_selection_snapshot(cx);
                    if selection.is_some() && selection != this.selection_dismissed {
                        this.selection_dismissed = selection;
                        this.clear_stop_confirmation(cx);
                        cx.stop_propagation();
                        cx.notify();
                    }
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
        let composer_disabled = self.dashboard_pending
            || self.submitted.is_some()
            || self
                .plan_answer
                .as_ref()
                .is_some_and(|answer| answer.saving.is_some())
            || self.edit_request_pending
            || self.edit_confirmation.is_some()
            || state.saved.pending_edit.is_some()
            || self
                .queue_editor
                .as_ref()
                .is_some_and(|editor| !editor.loaded || editor.saving);
        let glow_bounds =
            std::rc::Rc::new(std::cell::Cell::new(gpui::Bounds::<gpui::Pixels>::default()));
        let measured_glow_bounds = glow_bounds.clone();
        let writing_focused = self.input.focus_handle(cx).is_focused(window)
            && !self.composer_collapsed
            && !composer_disabled;
        let mut surface = div()
            .id("codex-conversation")
            .track_focus(&self.conversation_focus)
            .relative()
            .w_full()
            .when(self.dashboard_tray.is_none(), gpui::Styled::h_full)
            .min_h_0()
            .flex()
            .flex_col()
            .font_family(crate::fonts::UI_FAMILY)
            .text_size(rpx(TEXT_BODY))
            .text_color(c::FG())
            .child(super::components::composer_focus_glow(
                glow_bounds,
                writing_focused,
                !self.composer_collapsed && !composer_disabled,
                ("composer-focus-pulse", self.input.entity_id()),
                window,
                cx,
            ))
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
                focused_launch && self.dashboard_tray.is_none() && !self.transcript_only(),
                |surface| {
                    let project = grove_core::session_meta::multi_project_identity(
                        &state.saved.project,
                        &state.saved.roots,
                    )
                    .unwrap_or_else(|| state.saved.project.clone());
                    let worktree = self.checkout_context.as_ref().map_or_else(
                        || {
                            std::path::Path::new(&state.saved.cwd)
                                .file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        },
                        |(name, _)| name.clone(),
                    );
                    let identity = [project, worktree]
                        .into_iter()
                        .filter(|value| !value.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ");
                    surface.child(
                        div()
                            .id("codex-empty-intro")
                            .debug_selector(|| "codex-empty-intro".into())
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .px(rpx(content_inset))
                            .pt(rpx(SPACE_3XL))
                            .gap(rpx(SPACE_LG))
                            .text_center()
                            .child(
                                div()
                                    .text_size(rpx(TEXT_DISPLAY))
                                    .child("Ship fast. Break faster."),
                            )
                            .child(
                                div()
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_DIM())
                                    .child(identity),
                            ),
                    )
                },
            )
            .when(
                focused_launch && self.dashboard_tray.is_none(),
                gpui::StatefulInteractiveElement::overflow_y_scroll,
            )
            // Cmd+Enter resolves to the shell's global Zen action before Input's
            // secondary-enter binding. Own it only while the main draft has focus.
            .on_action(
                cx.listener(|this, _: &crate::keymap::ToggleZen, window, cx| {
                    if this.input.focus_handle(cx).is_focused(window) {
                        this.submit_modified_enter(cfg!(target_os = "macos"), window, cx);
                    } else {
                        cx.propagate();
                    }
                }),
            )
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                if this.input.focus_handle(cx).is_focused(window)
                    && (this.accept_first_suggestion(window, cx)
                        || this.accept_next_reply(window, cx))
                {
                    cx.stop_propagation();
                } else {
                    cx.propagate();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveLeft, window, cx| {
                if !this.input.focus_handle(cx).is_focused(window)
                    || !this.move_next_reply(true, window, cx)
                {
                    cx.propagate();
                } else {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveRight, window, cx| {
                if !this.input.focus_handle(cx).is_focused(window)
                    || !this.move_next_reply(false, window, cx)
                {
                    cx.propagate();
                } else {
                    cx.stop_propagation();
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
                if event.keystroke.key != "escape"
                    || event.keystroke.modifiers.modified()
                    || this.menu.is_some()
                    || this.model_from_card
                {
                    this.clear_stop_confirmation(cx);
                }
                if event.keystroke.key == "escape" && this.model_from_card {
                    this.model_from_card = false;
                    this.open_menu(ComposerMenu::Settings, window, cx);
                    this.card_model_focus.focus(window, cx);
                    cx.stop_propagation();
                } else if event.keystroke.key == "escape" && this.menu.is_some() {
                    let index = if this.menu == Some(ComposerMenu::Add) {
                        3
                    } else if this.menu == Some(ComposerMenu::Access) {
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
                if event.keystroke.key == "escape" && this.plan_answer.is_some() {
                    if this
                        .plan_answer
                        .as_ref()
                        .is_some_and(|a| a.saving.is_none())
                    {
                        this.cancel_plan_answer(window, cx);
                    }
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
                    && this.dismissed_suggestions.as_deref()
                        != Some(this.input.read(cx).value().as_ref())
                {
                    this.menu = None;
                    this.clear_stop_confirmation(cx);
                    this.dismissed_suggestions = Some(this.input.read(cx).value().to_string());
                    this.input.focus_handle(cx).focus(window, cx);
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape" && this.next_reply_text(window, cx).is_some() {
                    this.dismiss_next_reply(cx);
                    window.prevent_default();
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape" && this.stop_with_escape(event, cx) {
                    window.prevent_default();
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
                            .px(rpx(content_inset))
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
        let busy = state.status.busy();
        let stopping = busy && self.interrupt_requested_for_turn.is_some();
        // Keep immediate stop feedback above every attached panel, alongside live activity.
        if self.stop_confirmation.is_some() || stopping {
            surface = surface.child(
                div()
                    .w_full()
                    .flex_shrink_0()
                    .flex()
                    .justify_center()
                    .when(!minimap_entries.is_empty(), |row| {
                        row.pl(rpx(CHROME_CONTROL_H))
                    })
                    .px(rpx(content_inset))
                    .child(
                        div()
                            .id("codex-stop-confirmation")
                            .debug_selector(|| "codex-stop-confirmation".into())
                            .role(gpui::Role::Status)
                            .w_full()
                            .max_w(rpx(CHAT_MAX_W))
                            .flex()
                            .items_center()
                            .gap(rpx(SPACE_SM))
                            .py(rpx(SPACE_MD))
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG())
                            .child(if stopping {
                                super::status::mark(
                                    "escape-stopping",
                                    StatusKind::Working,
                                    ICON_SM,
                                    cx,
                                )
                            } else {
                                crate::icons::icon("stop", ICON_SM, c::FG_DIM()).into_any_element()
                            })
                            .child(div().flex_1().min_w_0().child(if stopping {
                                "Stopping agent…"
                            } else {
                                "Press Esc again to stop the agent"
                            })),
                    ),
            );
        }
        self.sync_status_cards(&state);
        let session_notice = self.session_notice(false, &state, window, cx);
        let (notice_above, notice_below) = if self.dashboard_tray.is_some() {
            (None, session_notice)
        } else {
            (session_notice, None)
        };
        let controls_attached = notice_above.is_some();
        let status_attached =
            controls_attached || !state.approvals.is_empty() || !state.elicitations.is_empty();
        let session_review = if state.plan_ready() {
            None
        } else {
            self.session_review(status_attached, &state, window, cx)
        };
        let has_review = session_review.is_some();
        let tasks_attached = status_attached || has_review;
        let lower_attached = tasks_attached || task_cache.list.is_some();
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
                    .track_scroll(&self.agent_controls_scroll)
                    .anchor_scroll(Some(self.agent_controls_anchor.clone()))
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_LG))
                    .mx(rpx(composer_tray_inset(window)))
                    .when(!state.approvals.is_empty(), |tray| {
                        tray.child(
                            composer_tray(window)
                                .rounded_none()
                                .mx_0()
                                .mb_0()
                                .pb(rpx(SPACE_LG))
                                .when(!controls_attached, |panel| {
                                    panel.rounded_t(rpx(RADIUS_PANEL))
                                })
                                .child(self.status_card_header(
                                    "approval-status-disclosure",
                                    "Approval needed",
                                    StatusKind::NeedsInput,
                                    cx,
                                ))
                                .when(
                                    !self.expanded.contains("approval-status-disclosure"),
                                    |panel| panel.child(self.approval_panel(&state, window, cx)),
                                ),
                        )
                    })
                    .when(!state.elicitations.is_empty(), |tray| {
                        tray.child(
                            composer_tray(window)
                                .rounded_none()
                                .mx_0()
                                .mb_0()
                                .pb(rpx(SPACE_LG))
                                .rounded_none()
                                .when(!controls_attached && state.approvals.is_empty(), |panel| {
                                    panel.rounded_t(rpx(RADIUS_PANEL))
                                })
                                .child(self.status_card_header(
                                    "elicitation-status-disclosure",
                                    "Input needed",
                                    StatusKind::NeedsInput,
                                    cx,
                                ))
                                .when(
                                    !self.expanded.contains("elicitation-status-disclosure"),
                                    |panel| panel.child(self.elicitation_panel(&state, window, cx)),
                                ),
                        )
                    }),
            )
        } else {
            None
        };
        self.previews.retain(|url, _| self.images.contains(url));
        let queued = self.queue_tray(
            lower_attached
                || !state.saved.rewind_drafts.is_empty()
                || (!self.composer_collapsed && self.suggestion_choices(&state, cx).is_some()),
            &state,
            window,
            cx,
        );
        let task_strip = task_cache.list.as_ref().map(|list| {
            let jumps = list.tasks.iter().map(|task| task.started_at).collect();
            self.task_strip(tasks_attached, list, state.status, jumps, window, cx)
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
                            (if self.dashboard_pending {
                                super::components::disabled_header_control(
                                    "codex-remove-image",
                                    "Remove attached image",
                                )
                            } else {
                                super::components::header_control(
                                    "codex-remove-image",
                                    "Remove attached image",
                                )
                            })
                            .tab_index(if self.dashboard_pending { -1 } else { 0 })
                            .tab_stop(!self.dashboard_pending)
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
                            .opacity(if self.dashboard_pending {
                                OPACITY_DISABLED
                            } else {
                                0.0
                            })
                            .when(!self.dashboard_pending, |control| {
                                control
                                    .group_hover("codex-image", |style| style.opacity(1.0))
                                    .focus_visible(|style| style.opacity(1.0))
                            })
                            .child(crate::icons::icon("close", ICON_MD, c::FG_DIM()))
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    if this.submitted.is_some() {
                                        return;
                                    }
                                    this.remove_image(index, window, cx);
                                },
                            )),
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
        let native_invocation =
            grove_core::describe_skill::is_invocation(self.input.read(cx).value().as_ref())
                || grove_core::review_skill::is_invocation(self.input.read(cx).value().as_ref());
        let dashboard_catalog_blocked = self.dashboard_tray.is_some() && !native_invocation && {
            let session = self.session.read(cx);
            session.catalog_loading || session.catalog_error.is_some()
        };
        let draft_disabled = dashboard_catalog_blocked
            || self.edit_confirmation.is_some()
            || self.edit_request_pending
            || state.saved.pending_edit.is_some()
            || state.status == codex::Status::Disconnected
            || (state.status == codex::Status::Starting
                && state.saved.provider != codex::Provider::Claude)
            || self.submitted.is_some()
            || self.pending_image_pastes > 0
            || self.pending_attachment_picker
            || stopping
            || (self.input.read(cx).value().trim().is_empty()
                && self.images.is_empty()
                && self.reference.is_none());
        let steer_available = active_turn_redirect_available(&state);
        let editing = self
            .queue_editor
            .as_ref()
            .is_some_and(|editor| editor.loaded);
        let primary_disabled = if let Some(answer) = &self.plan_answer {
            answer.saving.is_some() || self.input.read(cx).value().trim().is_empty()
        } else if editing {
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
        let primary_label = if self.plan_answer.is_some() {
            "Save answer · Enter · Does not start implementation"
        } else if editing {
            "Save queued message · Enter · Shift+Enter for a new line"
        } else if busy {
            "Queue · Enter · Send after the current turn finishes · Shift+Enter for a new line"
        } else {
            "Send · Enter · Start a new turn · Shift+Enter for a new line"
        };
        // An absent menu must not leave an empty flex child and an extra rail gap.
        let rewind_drafts = (!state.saved.rewind_drafts.is_empty()).then(|| {
            div()
                .id("codex-saved-rewind-drafts")
                .debug_selector(|| "codex-saved-rewind-drafts".into())
                .w_full()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_MD))
                .children(state.saved.rewind_drafts.iter().map(|draft| {
                    let id = draft.id.clone();
                    let disabled = self.edit_request_pending
                        || self.pending_image_pastes > 0
                        || self.pending_attachment_picker
                        || state.saved.pending_edit.is_some()
                        || state.status.busy()
                        || self.queue_editor.is_some()
                        || self.submitted.is_some();
                    div()
                        .id(gpui::SharedString::from(format!(
                            "saved-rewind-{}",
                            draft.id
                        )))
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_LG))
                        .p(rpx(SPACE_LG))
                        .rounded(rpx(RADIUS_GROUP))
                        .border_1()
                        .border_color(c::BORDER_SOFT())
                        .bg(c::BG_RAIL())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().text_size(rpx(TEXT_SMALL)).child("Saved draft"))
                                .child(
                                    div()
                                        .text_size(rpx(TEXT_SMALL))
                                        .text_color(c::FG_DIM())
                                        .truncate()
                                        .child(if draft.text.is_empty() {
                                            "Attachments or referenced selection".into()
                                        } else {
                                            draft.text.clone()
                                        }),
                                ),
                        )
                        .child(
                            form_action(
                                format!("restore-rewind-{}", draft.id),
                                "Restore draft",
                                false,
                                window,
                                cx,
                            )
                            .icon(
                                gpui_component::Icon::default()
                                    .path("icons/restore.svg")
                                    .size(rpx(ICON_SM)),
                            )
                            .disabled(disabled)
                            .debug_selector(|| "codex-restore-rewind-draft".into())
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if disabled
                                        || this.edit_request_pending
                                        || this.pending_image_pastes > 0
                                        || this.pending_attachment_picker
                                    {
                                        return;
                                    }
                                    this.dismiss_next_reply(cx);
                                    this.sync_composer_before_rewind(cx);
                                    this.edit_request_pending = true;
                                    this.rewind_attempt =
                                        Some(this.session.read(cx).state.rewind_attempts);
                                    this.session.update(cx, |session, cx| {
                                        session.state.clear_error();
                                        session.send(
                                            Action::RestoreRewindDraft { id: id.clone() },
                                            cx,
                                        );
                                    });
                                },
                            )),
                        )
                }))
        });
        let choices = (!self.composer_collapsed && self.suggestion_choices(&state, cx).is_some())
            .then(|| {
                self.composer_choices(
                    lower_attached || !state.saved.rewind_drafts.is_empty(),
                    &state,
                    window,
                    cx,
                )
            });
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
                                    .min_h(rpx(if self.composer_collapsed {
                                        CHROME_CONTROL_H
                                    } else {
                                        composer_min_h
                                    }))
                                    .when(!self.pending_attachment_picker, |input| {
                                        input.file_drop_overlay(
                                            crate::views::components::file_drop_overlay(),
                                        )
                                    })
                                    .disabled(composer_disabled)
                                    .aria_label(self.plan_answer.as_ref().map_or_else(
                                        || format!("Message {provider}"),
                                        |answer| format!("Your answer: {}", answer.prompt),
                                    ))
                                    .appearance(false)
                                    .bordered(false)
                                    .focus_bordered(false)
                                    .text_color(c::FG())
                                    .p_0(),
                            )
                            .when(self.next_reply_text(window, cx).is_some(), |field| {
                                let (position, count) =
                                    self.next_reply.position().unwrap_or((0, 0));
                                field.child(
                                    div()
                                        .id("next-reply-hint")
                                        .debug_selector(|| "next-reply-hint".into())
                                        .flex()
                                        .items_center()
                                        .flex_wrap()
                                        .gap(rpx(SPACE_LG))
                                        .text_size(rpx(TEXT_MICRO))
                                        .text_color(c::FG_DIM())
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(rpx(SPACE_SM))
                                                .child(self.next_reply_navigation(true, cx))
                                                .child(
                                                    div()
                                                        .id("next-reply-position")
                                                        .role(gpui::Role::Status)
                                                        .w(rpx(NEXT_REPLY_POSITION_W))
                                                        .text_center()
                                                        .child(format!("{position} / {count}")),
                                                )
                                                .child(self.next_reply_navigation(false, cx)),
                                        )
                                        .child("Tab to use · ← / → to switch · Esc to dismiss"),
                                )
                            })
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
        let composer_attached = lower_attached
            || rewind_drafts.is_some()
            || choices.is_some()
            || !self.queue_motion_rows.is_empty();
        let mut options = self.option_triggers(&state, window, cx).into_iter();
        let access_control = options.next();
        let model_control = options.next();
        let add_bounds = self.option_bounds[3].clone();
        let add_disabled = self.dashboard_pending
            || self.plan_answer.is_some()
            || self.submitted.is_some()
            || self.pending_attachment_picker
            || self.edit_request_pending
            || state.saved.pending_edit.is_some();
        let dashboard_status = self.render_dashboard_status(&state, window, cx);
        let mut surface =
            surface.child(
                div()
                    .id("codex-composer-rail")
                    .debug_selector(|| "codex-composer-rail".into())
                    // Short panes scroll attached controls instead of clipping them.
                    .min_h_0()
                    .max_h_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.composer_rail_scroll)
                    .w_full()
                    .when(focused_launch && self.dashboard_tray.is_none(), |rail| rail.pt(rpx(SPACE_3XL)))
                    .px(rpx(content_inset))
                    // Match the transcript's reserved navigation gutter so both
                    // centered columns retain the same edges at every pane width.
                    .when(!minimap_entries.is_empty(), |rail| {
                        rail.pl(rpx(content_inset + CHROME_CONTROL_H))
                    })
                    .pb(rpx(SPACE_3XL))
                    .flex()
                    .items_start()
                    .justify_center()
                    .child(
                        div()
                            .w_full()
                            .max_w(rpx(CHAT_MAX_W))
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_MD))
                            .when(has_review, gpui::Styled::gap_0)

                            .when_some(notice_above, gpui::ParentElement::child)
                            .when_some(agent_controls, gpui::ParentElement::child)
                            .when_some(session_review, gpui::ParentElement::child)
                            .when_some(task_strip, gpui::ParentElement::child)
                            .when_some(rewind_drafts, gpui::ParentElement::child)
                            .when_some(choices, gpui::ParentElement::child)
                            .when(!self.queue_motion_rows.is_empty(), |rail| rail.child(queued))
                            .when_some(dashboard_status, gpui::ParentElement::child)
                            .child(div().w_full().flex().flex_col().flex_shrink_0()
                            .children(self.render_reference_preview(composer_attached, window, cx))
                            .child(
                        super::components::composer_surface()
                            .relative()
                            .child(gpui::canvas(move |bounds, _, _| measured_glow_bounds.set(bounds), |_, (), _, _| {}).absolute().size_full())
                            .id("codex-composer")
                            .debug_selector(|| "codex-composer".into())
                            .max_w(rpx(CHAT_MAX_W))
                            .children(self.render_plan_answer_context(window, cx))
                            .child(body)
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(rpx(SPACE_LG))
                                    .when_some(model_control, gpui::ParentElement::child)
                                    .when_some(access_control, gpui::ParentElement::child)
                                    .child(div().flex_1())
                                    .child(
                                        div().flex().items_center().gap(rpx(SPACE_LG)).flex_shrink_0().ml_auto()
                                    .when_some(state.saved.context.filter(|usage| usage.window > 0), |row, usage| row.child(context_meter(usage)))
                                    .child(
                                        composer_icon("codex-add", "Add context", "paperclip", add_disabled, false)
                                            .track_focus(&self.option_focus[3])
                                            .relative()
                                            .child(gpui::canvas(move |rect, _, _| add_bounds.set(rect), |_, (), _, _| {}).absolute().size_full())
                                            .when(self.menu == Some(ComposerMenu::Add), |button| button.text_color(c::FG()))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                if !add_disabled { if this.menu == Some(ComposerMenu::Add) { this.close_menu(cx); } else { this.open_menu(ComposerMenu::Add, window, cx); } }
                                            }))
                                            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                                                if !add_disabled && action_key(event) { this.open_menu(ComposerMenu::Add, window, cx); cx.stop_propagation(); }
                                            }))
                                    )

                                            .when(self.plan_answer.is_some(), |row| row.child(
                                                transcript_action("save-plan-answer", "Save answer", true, window, cx)
                                                    .debug_selector(|| "save-plan-answer".into())
                                                    .icon(gpui_component::Icon::default().path("icons/check.svg").size(rpx(ICON_SM)))
                                                    .disabled(primary_disabled)
                                                    .on_click(cx.listener(|this, _, window, cx| this.save_plan_answer(window, cx)))
                                            ))
                                            .when(busy && self.plan_answer.is_none(), |row| row.child(
                                                composer_icon("codex-stop", if stopping { "Stopping the current turn" } else { "Stop · Interrupt the current turn" }, "stop", stopping || state.turn_id.is_none(), true)
                                                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                                                        if action_key(event) { this.interrupt(cx); cx.stop_propagation(); }
                                                    }))
                                                    .on_click(cx.listener(|this, _, _, cx| this.interrupt(cx)))
                                            ))
                                            .when(!busy && self.plan_answer.is_none(), |row| row.child(
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
                            )
                            .children(self.dashboard_tray.clone()))
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
                                    .line_height(gpui::phi())
                                    .text_color(c::FG_DIM())
                                    .when(!stopping && self.dashboard_tray.is_none() && (self.plan_answer.is_some() || self.submitted.is_some() || editing || busy), |hints| hints.child(super::components::shortcut_hint(if self.plan_answer.as_ref().is_some_and(|answer| answer.saving.is_some()) {
                                        "Saving answer…"
                                    } else if self.plan_answer.is_some() {
                                        "Enter to save answer"
                                    } else if self.submitted.is_some() {
                                        "Sending…"
                                    } else if editing {
                                        "Enter to save"
                                    } else if busy && steer_available {
                                        if cfg!(target_os = "macos") { "Enter to queue · Cmd+Enter to steer" } else { "Enter to queue · Ctrl+Shift+Enter to steer" }
                                    } else {
                                        "Enter to queue"
                                    })))
                            )
                            .when_some(notice_below, gpui::ParentElement::child)
                            ,
                    ),
            );
        let closing = self
            .closing_popup
            .as_ref()
            .filter(|popup| {
                self.menu.is_none() && popup.scope == composer_scope(&state) && !cx.reduce_motion()
            })
            .map(|popup| (popup.menu, popup.bounds));
        let exiting = closing.is_some();
        if let Some(menu) = self.menu.or_else(|| closing.map(|(menu, _)| menu)) {
            let popup = if menu == ComposerMenu::Settings {
                Some(self.model_effort_card(&state, exiting, window, cx))
            } else if menu == ComposerMenu::Model {
                if exiting {
                    self.closing_popup
                        .as_ref()
                        .and_then(|p| p.model_picker.clone())
                        .map(IntoElement::into_any_element)
                } else {
                    self.model_picker.clone().map(IntoElement::into_any_element)
                }
            } else {
                if exiting {
                    self.closing_popup
                        .as_ref()
                        .and_then(|p| p.popup.clone())
                        .map(IntoElement::into_any_element)
                } else {
                    self.popup.clone().map(IntoElement::into_any_element)
                }
            };
            if exiting {
                if let Some(popup) = self.closing_popup.as_ref().and_then(|p| p.popup.clone()) {
                    popup.update(cx, |popup, _| popup.set_passive(true));
                }
                if let Some(picker) = self
                    .closing_popup
                    .as_ref()
                    .and_then(|p| p.model_picker.clone())
                {
                    picker.update(cx, |picker, _| picker.passive = true);
                }
            }
            if let Some(popup) = popup {
                let index = match menu {
                    ComposerMenu::Add => 3,
                    ComposerMenu::Access => 2,
                    _ => 0,
                };
                let bounds =
                    closing.map_or_else(|| self.option_bounds[index].get(), |(_, bounds)| bounds);
                let gap = rpx(SPACE_LG).to_pixels(window.rem_size());
                // Anchor each picker to the stable outer edge of its toolbar group.
                let trailing = menu == ComposerMenu::Add;
                return surface
                    .child(gpui::deferred(
                        gpui::anchored()
                            .position_mode(gpui::AnchoredPositionMode::Window)
                            .anchor(if trailing {
                                gpui::Anchor::BottomRight
                            } else {
                                gpui::Anchor::BottomLeft
                            })
                            .position(gpui::point(
                                if trailing {
                                    bounds.right()
                                } else {
                                    bounds.left()
                                },
                                bounds.top() - gap,
                            ))
                            .snap_to_window_with_margin(rpx(SPACE_LG).to_pixels(window.rem_size()))
                            .child(
                                self.popup_motion.render(
                                    div()
                                        .id(if exiting {
                                            "codex-options-popup-exit"
                                        } else {
                                            "codex-options-popup"
                                        })
                                        .when(!exiting, |surface| {
                                            surface.debug_selector(|| "codex-options-popup".into())
                                        })
                                        .when(exiting, |surface| {
                                            surface
                                                .capture_any_mouse_down(|_, _, cx| {
                                                    cx.stop_propagation();
                                                })
                                                .capture_any_mouse_up(|_, _, cx| {
                                                    cx.stop_propagation();
                                                })
                                                .capture_key_down(|_, _, cx| cx.stop_propagation())
                                        })
                                        .child(popup),
                                    "composer-popup-rise",
                                    cx,
                                ),
                            ),
                    ))
                    .into_any_element();
            }
        }
        if self.edit_confirmation.is_some() {
            surface = surface.child(self.render_edit_confirmation(window, cx));
        }
        if self.file_browser.read(cx).visible {
            return div()
                .size_full()
                .min_h_0()
                .child(self.file_browser.clone())
                .into_any_element();
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

/// Deferred after the message action's canvas, so resize and scrolling use this frame's
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
    fn prune_transcript_controls(&mut self, transcript: &TranscriptRenderCache) {
        let revision = transcript.state.saved.items.revision();
        if self.focus_revision == Some(revision)
            && self.focus_thread == transcript.state.saved.thread_id
        {
            return;
        }
        if self.focus_thread != transcript.state.saved.thread_id {
            self.message_focus.clear();
            self.focus_subscriptions.clear();
        }
        self.focus_thread
            .clone_from(&transcript.state.saved.thread_id);
        self.focus_revision = Some(revision);
        let thread = transcript
            .state
            .saved
            .thread_id
            .as_deref()
            .unwrap_or_default();
        let prefix = format!("{thread}:");
        let mut valid: HashSet<String> = transcript
            .state
            .saved
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                format!(
                    "{thread}:{}",
                    item["id"]
                        .as_str()
                        .map_or_else(|| index.to_string(), str::to_owned)
                )
            })
            .collect();
        for group in &transcript.response_groups {
            valid.insert(group.key.clone());
            valid.extend(group.source_keys.iter().cloned());
        }
        let sources: HashMap<_, _> = transcript
            .state
            .saved
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                (
                    format!(
                        "{thread}:{}",
                        item["id"]
                            .as_str()
                            .map_or_else(|| index.to_string(), str::to_owned)
                    ),
                    item.clone(),
                )
            })
            .collect();
        let changed_code_owners: HashSet<_> = self
            .focus_sources
            .iter()
            .filter(|(owner, old)| {
                sources
                    .get(*owner)
                    .is_none_or(|new| !std::ptr::eq(old.as_ref(), new.as_ref()))
            })
            .map(|(owner, _)| owner.clone())
            .collect();
        self.focus_sources = sources;
        let count = transcript.state.saved.items.len();
        self.message_focus.retain(|key, _| {
            if key
                .rsplit_once(":code:")
                .is_some_and(|(owner, _)| changed_code_owners.contains(owner))
            {
                return false;
            }
            if let Some(boundary) = key.strip_prefix("fork:") {
                return boundary
                    .split(':')
                    .next()
                    .and_then(|v| v.parse::<usize>().ok())
                    .is_some_and(|index| index <= count);
            }
            for family in ["tool-row:", "tool-copy:", "work-header:"] {
                if let Some(row) = key.strip_prefix(family) {
                    return row
                        .strip_prefix(&prefix)
                        .and_then(|row| row.split(':').next())
                        .and_then(|v| v.parse::<usize>().ok())
                        .is_some_and(|index| index < count);
                }
            }
            if key.starts_with(&prefix) {
                let mut base = key.as_str();
                loop {
                    if valid.contains(base) {
                        return true;
                    }
                    let Some((parent, _)) = base.rsplit_once(':') else {
                        return false;
                    };
                    base = parent;
                }
            }
            true
        });
        self.focus_subscriptions
            .retain(|key, _| self.message_focus.contains_key(key));
    }

    fn ensure_control_focus(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        let focus = self
            .ensure_message_focus(key, window, cx)
            .tab_stop(true)
            .tab_index(0);
        self.message_focus.insert(key.to_owned(), focus.clone());
        focus
    }

    fn ensure_message_focus(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        if let Some(focus) = self.message_focus.get(key) {
            return focus.clone();
        }
        let focus = cx.focus_handle();
        self.focus_subscriptions.insert(
            key.to_owned(),
            (
                cx.on_focus_in(&focus, window, |_, _, cx| cx.notify()),
                cx.on_focus_out(&focus, window, |_, _, _, cx| cx.notify()),
            ),
        );
        self.message_focus.insert(key.to_owned(), focus.clone());
        focus
    }

    fn fork_control(
        &mut self,
        boundary: usize,
        item_id: Option<String>,
        restore_user: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let focus =
            self.ensure_control_focus(&format!("fork:{boundary}:{restore_user}"), window, cx);
        let enabled = item_id
            .as_deref()
            .and_then(|id| self.fork_boundary(id, restore_user, cx))
            .is_some();
        let keyboard_id = item_id.clone();
        let control = if enabled {
            header_control_with_label_tooltip("fork-message", "Fork session from here")
        } else {
            super::components::header_control_with_hover_tooltip(
                "fork-message",
                "Fork unavailable at this point",
                gpui::transparent_black(),
            )
        };
        control
            .id((
                if restore_user {
                    "fork-user-message"
                } else {
                    "fork-response"
                },
                boundary,
            ))
            .track_focus(&focus)
            .tab_index(0)
            .focus_visible(|style| style.bg(c::BG_HOVER()))
            .child(crate::icons::icon("git-branch", ICON_MD, c::FG_DIM()))
            .when(!enabled, |control| control.opacity(0.4))
            .on_click(cx.listener(move |this, _, _, cx| {
                if enabled {
                    if let Some(id) = &item_id {
                        this.fork_from_item(id, restore_user, cx);
                    }
                }
            }))
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if enabled && action_key(event) {
                    if let Some(id) = &keyboard_id {
                        this.fork_from_item(id, restore_user, cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .into_any_element()
    }

    fn response_copy_text(&self, group: &ResponseGroup) -> String {
        let text = group.copy_text();
        if self.review_scope.is_some() {
            grove_core::conversation_result::display_text(
                "review",
                "grove-review",
                "agentMessage",
                &text,
            )
        } else {
            description_conversation_text(self.description_scope.as_ref(), "agentMessage", &text)
        }
    }

    fn response_footer(
        &mut self,
        group: &ResponseGroup,
        projected: &codex::Snapshot,
        flow_margin: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        #[cfg(test)]
        let _phase = measured_slot::phases::Section::footer();
        #[cfg(test)]
        measured_slot::RENDERED_FOOTERS.with(|keys| {
            keys.borrow_mut().insert(group.key.clone());
        });
        let key = group.key.clone();
        if !self.message_focus.contains_key(&key) {
            let focus = cx.focus_handle();
            self.focus_subscriptions.insert(
                key.clone(),
                (
                    cx.on_focus_in(&focus, window, |_, _, cx| cx.notify()),
                    cx.on_focus_out(&focus, window, |_, _, _, cx| cx.notify()),
                ),
            );
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
        let copy_focus = self.ensure_control_focus(&format!("{key}:copy"), window, cx);
        let keyboard_key = key.clone();
        let copy_group = group.clone();
        let keyboard_group = group.clone();
        let mut button = if copied {
            header_control_frame("codex-message-copy", "Copied", c::BG_HOVER())
        } else {
            header_control_with_label_tooltip("codex-message-copy", "Copy entire response")
        }
        .track_focus(&copy_focus)
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
                let text = this.response_copy_text(&keyboard_group);
                this.copy_message(keyboard_key.clone(), text, cx);
                cx.stop_propagation();
            }
        }))
        .on_click(cx.listener(move |this, _, _, cx| {
            let text = this.response_copy_text(&copy_group);
            this.copy_message(key.clone(), text, cx);
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
        actions = actions.child(button).child(
            self.fork_control(
                group.end,
                group
                    .anchor_index
                    .and_then(|index| projected.saved.items.get(index))
                    .and_then(|item| item["id"].as_str())
                    .map(str::to_owned),
                false,
                window,
                cx,
            ),
        );
        let hover_key = group.key.clone();
        div()
            .id(gpui::SharedString::from(group.key.clone()))
            .w_full()
            .max_w(rpx(CHAT_MAX_W))
            .when(flow_margin, |footer| footer.mt(rpx(-SPACE_LG)))
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

    fn copy_code_block(
        &mut self,
        key: String,
        text: impl Into<gpui::SharedString>,
        cx: &mut Context<Self>,
    ) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.into().to_string()));
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

    fn source_item_index(&self, id: &str, cx: &App) -> Option<usize> {
        let state = &self.session.read(cx).state;
        if let Some(cache) = self
            .transcript_cache
            .as_ref()
            .filter(|cache| cache.source.saved.items.revision() == state.saved.items.revision())
        {
            return cache
                .id_index
                .get(id)
                .copied()
                .filter(|index| *index < state.saved.items.len());
        }
        state.saved.items.iter().position(|item| item["id"] == id)
    }

    fn fork_boundary(&self, id: &str, restore_user: bool, cx: &App) -> Option<usize> {
        if self.transcript_only() {
            return None;
        }
        let boundary = self.source_item_index(id, cx)? + usize::from(!restore_user);
        self.session
            .read(cx)
            .state
            .can_fork_at(boundary, restore_user)
            .then_some(boundary)
    }

    fn fork_from_item(&self, id: &str, restore_user: bool, cx: &mut Context<Self>) {
        if let Some(boundary) = self.fork_boundary(id, restore_user, cx) {
            cx.emit(CodexViewEvent::ForkRequested(boundary, restore_user));
        }
    }

    fn remove_reference(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dashboard_pending {
            return;
        }
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
        if self.dashboard_pending {
            return;
        }
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
            || self.queue_editor.is_some()
            || self.plan_answer.is_some()
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
            header_control_with_label_tooltip("codex-selection-reply", "Reply to selection")
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
        attached_above: bool,
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
                .when(!attached_above, |preview| {
                    preview.rounded_t(rpx(RADIUS_PANEL))
                })
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
                            (if self.dashboard_pending {
                                super::components::disabled_header_control(
                                    "codex-remove-reference",
                                    "Remove reference",
                                )
                            } else {
                                super::components::header_control(
                                    "codex-remove-reference",
                                    "Remove reference",
                                )
                            })
                            .debug_selector(|| "codex-remove-reference".into())
                            .track_focus(
                                &self
                                    .reference_remove_focus
                                    .clone()
                                    .tab_index(if self.dashboard_pending { -1 } else { 0 })
                                    .tab_stop(!self.dashboard_pending)
                                    .tab_stop(!self.dashboard_pending),
                            )
                            .tab_index(if self.dashboard_pending { -1 } else { 0 })
                            .tab_stop(!self.dashboard_pending)
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
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.remove_reference(window, cx);
                                },
                            )),
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
        let native_command = self.input.read(cx).value().to_string();
        if grove_core::review_skill::is_invocation(&native_command) {
            if self.reference.is_some() || !self.images.is_empty() || self.pending_image_pastes > 0
            {
                self.session.update(cx, |session, cx| { session.state.error = Some("Review changes uses the checkout diff. Remove attachments or the quoted reference before invoking it; your draft is preserved.".into()); cx.notify(); });
                return true;
            }
            let Some(preference) = grove_core::review_skill::invocation(&native_command) else {
                self.session.update(cx, |session, cx| {
                    session.state.error = Some(grove_core::review_skill::USAGE.into());
                    cx.notify();
                });
                return true;
            };
            self.session.update(cx, |session, cx| {
                session.state.clear_error();
                cx.notify();
            });
            cx.emit(CodexViewEvent::ReviewChanges(
                preference,
                self.input.focus_handle(cx),
            ));
            // Keep this command draft intact: native actions do not send or replace a provider prompt.
            return true;
        }
        if grove_core::describe_skill::is_invocation(&native_command) {
            if self.reference.is_some() || !self.images.is_empty() || self.pending_image_pastes > 0
            {
                self.session.update(cx, |session, cx| { session.state.error = Some("Describe changes uses the checkout diff. Remove attachments or the quoted reference before invoking it; your draft is preserved.".into()); cx.notify(); });
                return true;
            }
            let Some(preference) = grove_core::describe_skill::invocation(&native_command) else {
                self.session.update(cx, |session, cx| {
                    session.state.error = Some(grove_core::describe_skill::USAGE.into());
                    cx.notify();
                });
                return true;
            };
            self.session.update(cx, |session, cx| {
                session.state.clear_error();
                cx.notify();
            });
            cx.emit(CodexViewEvent::DescribeChanges(
                preference,
                self.input.focus_handle(cx),
            ));
            // Keep this command draft intact: native actions do not send or replace a provider prompt.
            return true;
        }
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

    fn toggle_expansion(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(key) {
            self.expanded.insert(key.to_owned());
        }
        cx.notify();
    }
}

/// During a turn only its latest activity is visible, including between calls.
/// Completion restores the work disclosure's full history.
#[cfg(test)]
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
#[derive(Clone, Debug)]
struct ResponseGroup {
    key: String,
    end: usize,
    prose: Vec<usize>,
    source_keys: Vec<String>,
    copy_text: std::cell::OnceCell<String>,
    copy_sources: Vec<gpui::SharedString>,
    anchor_index: Option<usize>,
    received_at: Option<u64>,
}

impl ResponseGroup {
    fn copy_text(&self) -> String {
        self.copy_text
            .get_or_init(|| {
                self.copy_sources
                    .iter()
                    .map(std::convert::AsRef::as_ref)
                    .collect::<Vec<_>>()
                    .join("\n\n")
            })
            .clone()
    }
}

#[cfg(test)]
fn response_groups<T: std::borrow::Borrow<serde_json::Value>>(
    items: &[T],
    thread_id: Option<&str>,
) -> Vec<ResponseGroup> {
    response_groups_from(items, thread_id, 0, None)
}

fn response_groups_from<T: std::borrow::Borrow<serde_json::Value>>(
    items: &[T],
    thread_id: Option<&str>,
    restart: usize,
    prepared: Option<&[Rc<PreparedItem>]>,
) -> Vec<ResponseGroup> {
    let mut groups = Vec::new();
    let mut start = restart;
    // Compute the last usable fork anchor once, including idless trailing items.
    let mut anchors = Vec::with_capacity(items.len() + 1);
    let mut anchor = None;
    anchors.push(anchor);
    for (index, item) in items.iter().enumerate() {
        if item.borrow()["id"].is_string() {
            anchor = Some(index);
        }
        anchors.push(anchor);
    }
    for end in (restart..=items.len())
        .filter(|index| *index == items.len() || items[*index].borrow()["type"] == "userMessage")
    {
        let prose = (start..end)
            .filter(|index| {
                items[*index].borrow()["type"] == "agentMessage"
                    && !prepared_item_text(items, prepared, *index)
                        .trim()
                        .is_empty()
            })
            .collect::<Vec<_>>();
        if let Some(last) = prose.last() {
            let first = (start..end)
                .find(|index| items[*index].borrow()["type"] == "agentMessage")
                .unwrap_or(*last);
            let source_id = |index: usize| {
                items[index].borrow()["id"]
                    .as_str()
                    .map_or_else(|| index.to_string(), str::to_owned)
            };
            let key = format!(
                "{}:response:{}",
                thread_id.unwrap_or_default(),
                source_id(first)
            );
            groups.push(ResponseGroup {
                anchor_index: anchors[end],
                source_keys: prose
                    .iter()
                    .map(|index| format!("{key}:source:{}", source_id(*index)))
                    .collect(),
                copy_text: std::cell::OnceCell::default(),
                copy_sources: prose
                    .iter()
                    .map(|index| {
                        prepared.map_or_else(
                            || transcript_item_text(items[*index].borrow()).into(),
                            |prepared| prepared[*index].text.clone(),
                        )
                    })
                    .collect(),
                received_at: items[*last].borrow()["groveReceivedAt"].as_u64(),
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
#[cfg(test)]
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
    Some((
        work_key_at(state, index),
        work_duration_label(active, duration),
        active,
    ))
}

fn work_duration_label(active: bool, duration: Option<u64>) -> String {
    let verb = if active { "Working" } else { "Worked" };
    duration.map_or_else(
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
    )
}

/// Only parent commentary and tools belong to the work disclosure.
#[cfg(test)]
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
    !grove_core::activity::is_internal(item)
        && !matches!(
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
fn task_cache_key<T: std::borrow::Borrow<serde_json::Value>>(items: &[T]) -> u64 {
    let mut hasher = rustc_hash::FxHasher::default();
    items.len().hash(&mut hasher);
    for (index, item) in items.iter().enumerate() {
        let item = item.borrow();
        if tasks::is_checklist(item) {
            index.hash(&mut hasher);
            hash_transcript_value(item, &mut hasher);
        }
    }
    hasher.finish()
}

fn task_cache<T: std::borrow::Borrow<serde_json::Value>>(items: &[T], key: u64) -> TaskCache {
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
    let id = id.into();
    if status == TaskStatus::InProgress && phase == TaskPhase::Running && !cx.reduce_motion() {
        return super::components::isolated_animation(id.clone(), move |_, cx| {
            animated_task_mark_inner(id.clone(), status, phase, cx)
        });
    }
    animated_task_mark_inner(id, status, phase, cx)
}

fn animated_task_mark_inner(
    id: gpui::ElementId,
    status: TaskStatus,
    phase: TaskPhase,
    cx: &App,
) -> gpui::AnyElement {
    let selector = match &id {
        gpui::ElementId::Name(name) if name.as_ref() == "codex-task-strip-mark" => {
            "codex-task-header-mark".to_owned()
        }
        gpui::ElementId::NamedInteger(name, index) if name.as_ref() == "codex-task-row-mark" => {
            format!("codex-task-row-status-{index}")
        }
        _ => "codex-task-status".to_owned(),
    };
    let slot = div()
        .debug_selector(move || selector.clone())
        .size(rpx(ICON_SM))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center();
    if status == TaskStatus::InProgress && phase == TaskPhase::Running && !cx.reduce_motion() {
        slot.with_animation(
            id,
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
        || (state
            .saved
            .items
            .last()
            .is_some_and(|item| is_tool_item(item))
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

/// Choose the operation identity independently of whether its output is expanded.
fn tool_activity_icon(item: &serde_json::Value) -> &'static str {
    if tool_failed(item) {
        return "message-error";
    }
    match item["type"].as_str().unwrap_or_default() {
        "reasoning" => "brain",
        "commandExecution" => "terminal",
        "fileChange" => "edit",
        "webSearch" => "search",
        "plan" => "list",
        _ => {
            let action = item["tool"]
                .as_str()
                .unwrap_or_default()
                .rsplit("__")
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            match action.as_str() {
                "bash" | "shell" | "exec_command" | "write_stdin" => "terminal",
                "read" | "read_file" => "file",
                "write" | "write_file" | "edit" | "multiedit" | "apply_patch" => "edit",
                "search" | "grep" | "glob" | "websearch" => "search",
                "fetch" | "webfetch" => "external-link",
                "think" | "thought" | "thinking" => "brain",
                "wait" | "wait_agent" | "sleep" => "status-limited",
                _ => "tool",
            }
        }
    }
}

#[cfg(test)]
#[test]
fn tool_icons_distinguish_codex_and_claude_operations() {
    use serde_json::json;
    // Normalized Codex items and Claude tool names use the same renderer.
    for (item, expected) in [
        (json!({"type":"commandExecution"}), "terminal"),
        (json!({"type":"fileChange"}), "edit"),
        (json!({"type":"webSearch"}), "search"),
        (json!({"type":"reasoning"}), "brain"),
        (json!({"type":"mcpToolCall", "tool":"Bash"}), "terminal"),
        (json!({"type":"mcpToolCall", "tool":"Read"}), "file"),
        (json!({"type":"mcpToolCall", "tool":"Edit"}), "edit"),
        (
            json!({"type":"mcpToolCall", "tool":"mcp__docs__search"}),
            "search",
        ),
        (
            json!({"type":"mcpToolCall", "tool":"custom_integration"}),
            "tool",
        ),
        (
            json!({"type":"mcpToolCall", "tool":"Read", "is_error":true}),
            "message-error",
        ),
    ] {
        assert_eq!(tool_activity_icon(&item), expected, "{item}");
    }
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
    state.can_redirect_active_turn()
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
        .bg(c::CODE_FILL())
        .border_1()
        .border_color(c::BORDER_STRONG())
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

/// Rank the whole catalog before applying the popup limit, so an early scattered
/// match cannot hide a later exact or prefix match. Stable sorting preserves the
/// catalog order for ties and for the empty query.
fn fuzzy_completion_choices(
    choices: impl IntoIterator<Item = (String, String)>,
    query: &str,
) -> Vec<(String, String)> {
    let mut matches = choices
        .into_iter()
        .filter_map(|choice| {
            let name = choice.0.trim_start_matches(['/', '$']);
            crate::launcher::fuzzy_score(query, name, "", "").map(|score| (score, choice))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|(a, _), (b, _)| b.cmp(a));
    matches.into_iter().map(|(_, choice)| choice).collect()
}

fn skill_choices(state: &codex::Snapshot, query: &str) -> Vec<(String, String)> {
    fuzzy_completion_choices(
        grove_core::describe_skill::completion('$', "")
            .into_iter()
            .chain(grove_core::review_skill::completion('$', ""))
            .chain(state.skills.iter().filter_map(|skill| {
                let name = skill["name"].as_str()?;
                (name != grove_core::describe_skill::NAME
                    && name != grove_core::review_skill::NAME
                    && (state.saved.provider != codex::Provider::Claude || !host_command(name))
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
            })),
        query,
    )
    .into_iter()
    .take(8)
    .collect()
}

/// Host commands first, then (for Claude) the CLI's own commands Grove does not host.
fn slash_choices(state: &codex::Snapshot, query: &str) -> Vec<(String, String)> {
    let mut choices: Vec<(String, String)> = [
        (
            "/describe-changes",
            "Describe the selected checkout or attached PR",
        ),
        ("/review", "Review changes for actionable defects"),
        ("/model", "Choose model"),
        ("/effort", "Choose reasoning effort"),
        ("/access", "Choose access level"),
        ("/stop", "Interrupt the current turn"),
        ("/compact", "Compact conversation context"),
        ("/queue", "Send the next queued prompt"),
        ("/clear", "Start a new conversation"),
    ]
    .into_iter()
    .map(|(command, label)| (command.into(), format!("{command} · {label}")))
    .collect();
    if state.saved.provider == codex::Provider::Claude {
        choices.extend(state.skills.iter().filter_map(|skill| {
            let name = skill["name"].as_str()?;
            (!host_command(name) && skill["enabled"].as_bool() != Some(false)).then(|| {
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
    fuzzy_completion_choices(choices, query)
}

fn host_command(name: &str) -> bool {
    matches!(
        name,
        "review"
            | "describe-changes"
            | "model"
            | "effort"
            | "access"
            | "stop"
            | "queue"
            | "clear"
            | "compact"
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

// Snapshot/schema transport is retained for recovery and model context, while
// the conversation presents its user-facing command and completion prose.
fn description_conversation_text(
    scope: Option<&grove_core::change_description::Scope>,
    kind: &str,
    text: &str,
) -> String {
    let Some(scope) = scope else {
        return text.into();
    };
    if kind == "userMessage"
        && text.starts_with("Describe ONLY the supplied immutable change snapshot.")
        && text.contains("\n\nGrove request identity: ")
    {
        return match scope {
            grove_core::change_description::Scope::Local => "/describe-changes local".into(),
            grove_core::change_description::Scope::PullRequest { url } => {
                format!("/describe-changes {url}")
            }
        };
    }
    if kind != "agentMessage" {
        return text.into();
    }
    let mut result = Vec::new();
    let mut hidden = false;
    let mut other_fence = false;
    for line in text.lines() {
        let marker = line.trim();
        if hidden {
            if marker == "```" {
                hidden = false;
            }
            continue;
        }
        if other_fence {
            result.push(line);
            if marker == "```" {
                other_fence = false;
            }
            continue;
        }
        if marker == "```grove-description" {
            hidden = true;
            continue;
        }
        if marker.starts_with("```") {
            other_fence = true;
        }
        result.push(line);
    }
    let result = result.join("\n").trim().to_owned();
    if result.is_empty() && text.contains("```grove-description") {
        "Change description data".into()
    } else {
        result
    }
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
/// Search only the active checkout, including untracked but non-ignored files.
fn file_choices(cwd: &str, query: &str) -> Vec<String> {
    let root = std::path::Path::new(cwd);
    let output = std::process::Command::new("git")
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            ".",
        ])
        .current_dir(root)
        .output();
    let mut paths = std::collections::BTreeSet::new();
    if let Some(output) = output.ok().filter(|output| output.status.success()) {
        for path in output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
        {
            let path = String::from_utf8_lossy(path).into_owned();
            if root.join(&path).is_file() {
                paths.insert(path);
            }
        }
    } else {
        // Ordinary directories also support completion. Do not follow directory
        // symlinks, which may escape the checkout or form cycles.
        let mut directories = vec![std::path::PathBuf::new()];
        while let Some(directory) = directories.pop() {
            let Ok(entries) = fs_err::read_dir(root.join(&directory)) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                let name = entry.file_name();
                if name.to_string_lossy().starts_with('.') {
                    continue;
                }
                let relative = directory.join(name);
                if kind.is_dir() {
                    directories.push(relative);
                } else if kind.is_file() {
                    paths.insert(relative.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    // Keep directory completion available alongside recursive filename lookup.
    for path in paths.clone() {
        let mut parent = std::path::Path::new(&path).parent();
        while let Some(directory) = parent.filter(|p| !p.as_os_str().is_empty()) {
            paths.insert(format!("{}/", directory.to_string_lossy()));
            parent = directory.parent();
        }
    }
    let mut matches = paths
        .into_iter()
        .filter_map(|path| {
            let name = path
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(&path);
            let score = crate::launcher::fuzzy_score(query, name, "", "")
                .map(|score| score + 200)
                .or_else(|| crate::launcher::fuzzy_score(query, &path, "", ""))?;
            Some((score, path))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|(a_score, a), (b_score, b)| b_score.cmp(a_score).then_with(|| a.cmp(b)));
    matches.into_iter().take(8).map(|(_, path)| path).collect()
}

#[cfg(test)]
mod tests {
    mod composer_input;
    mod composer_layout;
    mod configuration_controls;
    mod conversation_layout;
    mod dashboard_handoff;
    mod draft_recovery;
    mod message_rendering;
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
                        ].into();
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
                        ].into();
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
                ].into();
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
        // Channel-backed provider-shaped views: no provider launch or script activation.
        let checkout = std::env::temp_dir().join(format!(
            "grove-snippet-metal-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs_err::create_dir_all(&checkout).unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&checkout)
            .status()
            .unwrap()
            .success());
        let checkout = checkout.canonicalize().unwrap();
        let cwd = checkout.to_string_lossy().into_owned();
        for available in [true, false] {
            cx.update(|cx| {
                cx.set_global(crate::settings::SettingsState::new(
                    grove_core::storage::Store {
                        projects: if available {
                            vec![grove_core::storage::Project {
                                name: "Snippet Metal fixture".into(),
                                path: cwd.clone(),
                                scripts: grove_core::storage::ProjectScripts::default(),
                                archived: false,
                                worktree_dir: None,
                            }]
                        } else {
                            vec![]
                        },
                        ..Default::default()
                    },
                ));
                cx.set_global(crate::zoom::CurrentPtyDims::default());
                gpui_component::Theme::global_mut(cx).font_size = gpui::px(16.0);
            });
            for width in [1024.0, 600.0] {
                for provider in [codex::Provider::Codex, codex::Provider::Claude] {
                    let mut state = snapshot();
                    state.saved.provider = provider;
                    state.saved.cwd = cwd.clone();
                    state.saved.items = vec![
                        json!({"type":"agentMessage", "id":"snippet", "text":"Copy this code:\n\n```sh\n  cargo test --workspace\n```", "groveCompleted":true}),
                    ].into();
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
                                let session = cx
                                    .new(|cx| CodexSession::from_connection(state, connection, cx));
                                let runtime = cx.new(crate::runtime::Runtime::new);
                                let chat = cx.new(|cx| CodexView::new(session, window, cx));
                                chat.update(cx, |view, cx| view.set_script_runtime(runtime, cx));
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
                                        "  cargo test --workspace\n",
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
                                assert_eq!(window.rem_size(), gpui::px(16.0));
                                assert!(
                                    retained.as_ref().unwrap().read(cx).script_runtime.is_some(),
                                    "both availability states must render the generated Run path"
                                );
                                window.simulate_next_frame(cx);
                                let _ = window.draw(cx);
                            })
                            .unwrap();
                        }
                        let path =
                        format!("/tmp/grove-snippet-copy-{provider:?}-{width:.0}-available-{available}-copied-{copied}.png");
                        cx.capture_screenshot(window.into())
                            .unwrap()
                            .save(&path)
                            .unwrap();
                        eprintln!("Headless Metal provider-shaped fixture (not live provider): provider={provider:?} width={width:.0} rem=16 generated_runtime=true available={available} copied={copied} path={path}");
                    }
                    cx.update_window(window.into(), |_, window, _| window.remove_window())
                        .unwrap();
                }
            }
        }
        fs_err::remove_dir_all(checkout).unwrap();
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
                        let baseline = baseline_heights[&(kind, width as u32)].into();
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
                ].into();
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

    #[test]
    #[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
    #[cfg(target_os = "macos")]
    fn plan_approval_receipt_native_snapshots() {
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
        for (width, rem) in [(1024.0, 16.0), (400.0, 16.0), (400.0, 20.0)] {
            for provider in [codex::Provider::Codex, codex::Provider::Claude] {
                let (handle, _actions) = Handle::channel();
                let (_updates, receive) = mpsc::channel();
                let mut captured = None;
                let window = cx
                    .open_window(
                        gpui::size(gpui::px(width), gpui::px(900.0)),
                        |window, cx| {
                            gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                            let session = cx.new(|cx| {
                                CodexSession::from_connection(
                                    approval_receipt_snapshot(provider),
                                    Connection {
                                        handle,
                                        updates: receive.into(),
                                    },
                                    cx,
                                )
                            });
                            let chat = cx.new(|cx| CodexView::new(session, window, cx));
                            captured = Some(chat.clone());
                            cx.new(|cx| {
                                gpui_component::Root::new(chat, window, cx)
                                    .bordered(false)
                                    .bg(c::BG())
                            })
                        },
                    )
                    .unwrap();
                cx.run_until_parked();
                for details in [false, true] {
                    cx.update_window(window.into(), |_, window, cx| {
                        captured.as_ref().unwrap().update(cx, |view, cx| {
                            if details {
                                view.expanded
                                    .insert("approval-details-approval-message".into());
                            }
                            cx.notify();
                        });
                        let _ = window.draw(cx);
                    })
                    .unwrap();
                    cx.run_until_parked();
                    let path = format!(
                        "/tmp/grove-plan-approval-{provider:?}-{width:.0}-{rem:.0}-{}.png",
                        if details { "details" } else { "receipt" }
                    );
                    cx.capture_screenshot(window.into())
                        .unwrap()
                        .save(&path)
                        .unwrap();
                    eprintln!("Native plan approval receipt: {path}");
                }
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
    }

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
                ].into();
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
                ].into();
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

    include!("codex_view/performance.rs");
    include!("codex_view/native_hit_geometry.rs");
    include!("codex_view/live_performance.rs");

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
        let request =
            grove_core::next_reply::Request::capture(&view.session.read(cx).state).unwrap();
        view.next_reply_seen = Some(request.key.clone());
        view.next_reply_request = Some(request.key);
        view.next_reply = grove_core::next_reply::Candidate::pending();
        view.next_reply.resolve(vec![proposal.into()]);
    }

    #[test]
    #[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
    #[cfg(target_os = "macos")]
    fn question_receipt_native_snapshots() {
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
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            for (width, rem) in [(1024.0, 16.0), (520.0, 16.0), (520.0, 24.0)] {
                cx.update(|cx| gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem));
                let mut state = snapshot();
                state.saved.provider = provider;
                let (handle, _actions) = Handle::channel();
                let (_updates, receive) = mpsc::channel();
                let connection = Connection {
                    handle,
                    updates: receive.into(),
                };
                let mut retained = None;
                let window = cx
                    .open_window(
                        gpui::size(gpui::px(width), gpui::px(1000.0)),
                        |window, cx| {
                            window.set_rem_size(gpui::px(rem));
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
                // Initial synchronization clears state from the preceding session.
                // Insert the acknowledged receipt only after that first render.
                cx.run_until_parked();
                cx.update_window(window.into(), |_, window, cx| {
                    let _ = window.draw(cx);
                    retained.as_ref().unwrap().update(cx, |view, cx| {
                        view.question_receipts = vec![wrapped_question_receipt()];
                        cx.notify();
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
                    assert_eq!(window.rem_size(), gpui::px(rem));
                    let view = retained.as_ref().unwrap().read(cx);
                    assert_eq!(
                        view.question_receipts.len(),
                        1,
                        "native fixture must retain its receipt"
                    );
                })
                .unwrap();
                let path =
                    format!("/tmp/grove-question-receipt-{provider:?}-{width:.0}-{rem:.0}.png");
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(&path)
                    .unwrap();
                eprintln!("Native question receipt fixture: {path}");
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
    }

    /// Real native task rendering with synthetic provider snapshots; no live transport.
    #[test]
    #[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
    #[cfg(target_os = "macos")]
    fn task_strip_native_snapshots() {
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
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            for (width, rem) in [(1024., 16.), (600., 16.), (600., 20.)] {
                cx.update(|cx| gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem));
                let mut state = snapshot();
                state.saved.provider = provider;
                state.status = Status::Working;
                state.saved.items = vec![
                    json!({"id":"u","type":"userMessage","content":[{"text":"Align the task register"}]}),
                    json!({"id":"tasks","type":"plan","explanation":"Keep task labels aligned with the header while preserving jump actions.","plan":[
                        {"step":"Inspect the existing task dock","status":"completed"},
                        {"step":"Align task text and stable status slots","status":"inProgress"},
                        {"step":"Verify narrow widths and larger text","status":"pending"}
                    ]}),
                ].into();
                let (handle, _actions) = Handle::channel();
                let (_updates, receive) = mpsc::channel();
                let connection = Connection {
                    handle,
                    updates: receive.into(),
                };
                let window = cx
                    .open_window(gpui::size(gpui::px(width), gpui::px(700.)), |window, cx| {
                        window.set_rem_size(gpui::px(rem));
                        let session =
                            cx.new(|cx| CodexSession::from_connection(state, connection, cx));
                        let chat = cx.new(|cx| CodexView::new(session, window, cx));
                        cx.new(|cx| {
                            gpui_component::Root::new(chat, window, cx)
                                .bordered(false)
                                .bg(c::BG())
                        })
                    })
                    .unwrap();
                for _ in 0..3 {
                    cx.run_until_parked();
                    cx.update_window(window.into(), |_, window, cx| {
                        window.simulate_next_frame(cx);
                        let _ = window.draw(cx);
                    })
                    .unwrap();
                }
                let path = format!("/tmp/grove-task-strip-{provider:?}-{width:.0}-{rem:.0}.png");
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(&path)
                    .unwrap();
                eprintln!("Native task strip fixture: {path}");
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
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
}

#[cfg(test)]
mod transcript_signature_tests {
    use super::transcript_content_signature;
    use serde_json::json;

    #[test]
    fn content_signature_tracks_equal_length_edits_optimistic_items_images_and_threads() {
        let mut items: grove_core::codex::TranscriptItems =
            vec![json!({"id":"answer", "type":"agentMessage", "text":"one"})].into();
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
