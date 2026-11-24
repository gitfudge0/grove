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
use base64::engine::general_purpose::STANDARD;
#[cfg(test)]
use base64::Engine as _;
use gpui::{div, prelude::*, App, Context, Entity, FocusHandle, Focusable, Subscription, Window};
#[cfg(test)]
use gpui_component::WindowExt as _;
use gpui_component::{
    input::{
        IndentInline, Input, InputEvent, InputState, MoveDown, MoveLeft, MoveRight, MoveUp,
        TextDecoration, TextDecorationCollection, TextReplacement,
    },
    menu::{PopupMenu, PopupMenuItem},
    text::{TextSelectionSnapshot, TextView, TextViewState, TextViewStyle},
    Disableable,
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

mod composer_markdown;
mod composer_rail;
mod conversation_surface;
mod frame_state;
mod minimap;
mod mount;
mod native_commands;
mod next_reply;
mod optimistic_messages;
mod references;
mod responses;
mod session_status;
mod task_strip;
#[cfg(test)]
mod tests;
mod transcript_actions;
mod transcript_surface;
mod work_activity;

use attachments::image_from_url;
pub(super) use composer_markdown::{composer_markdown_decorations, composer_markdown_replacements};
use composer_rail::ComposerFrame;
use composer_suggestions::{host_command, trailing_trigger};
use message_rows::description_conversation_text;
use minimap::{
    message_minimap_entries_from, prepared_item_text, relative_message_time, transcript_item_text,
    MessageMinimapEntry, TranscriptMinimap,
};
use optimistic_messages::{project_pending_bubbles, BubbleEntrance};
use references::{reference_prompt, reference_prompt_copy, reference_prompt_markdown};
use responses::{
    hash_transcript_value, response_groups_from, transcript_content_signature, ResponseGroup,
};
use session_status::context_meter;
use task_strip::{
    task_cache, task_cache_key, TaskCache, TASK_LABEL_INSET, TASK_MARK_INSET, TASK_STRIP_HEAD_H,
};
use transcript_surface::{ComposerPlacement, InteractiveTranscript, TranscriptSurface};
use work_activity::{
    active_work_start, compacting, compaction_divider, is_tool_item, live_activity,
    tool_activity_icon, tool_activity_label, tool_activity_summary, tool_detail,
    tool_expansion_key, tool_failed, tool_is_running, tool_summary, work_duration_label,
};

#[cfg(test)]
use composer_suggestions::{completion_trigger, file_choices, skill_choices, slash_choices};
#[cfg(test)]
use minimap::{message_minimap_entries, minimap_excerpt, MINIMAP_BUILDS};
#[cfg(test)]
use responses::response_groups;
#[cfg(test)]
use session_status::{context_detail, context_tone, review_overview};
#[cfg(test)]
use task_strip::{
    task_header_status, task_mark_phase, task_phase, task_summary, TaskSummary,
    SPINNER_TICKS_PER_FRAME,
};
#[cfg(test)]
use work_activity::{
    tool_visible, work_commentary, work_has_parent_activity, work_header_at, work_is_active_at,
    work_key_at,
};

const CODE_COPY_FEEDBACK_MS: u64 = 1200;
const NEXT_REPLY_POSITION_W: f32 = 32.0;
const STOP_CONFIRMATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

struct StopConfirmation {
    thread_id: Option<String>,
    turn_id: String,
    started: std::time::Instant,
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
    inspect_focus: FocusHandle,
    details_scroll: gpui::ScrollHandle,
    menu_focus: FocusHandle,
    menu: Option<(Entity<PopupMenu>, Subscription)>,
    inspecting: bool,
    feedback_open: bool,
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
    answers: Vec<QuestionReceiptAnswer>,
    expanded: bool,
    details_focus: FocusHandle,
}

#[derive(Clone, Debug)]
struct QuestionReceiptAnswer {
    header: String,
    prompt: String,
    values: Vec<String>,
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
    question_answer_values(question, answers).join(", ")
}

fn question_answer_values(
    question: &codex::Question,
    answers: &codex::QuestionAnswers,
) -> Vec<String> {
    if !question_has_answer(question, answers) {
        vec!["Not answered".into()]
    } else if question.secret {
        vec!["Provided securely".into()]
    } else {
        let mut values = answers[&question.id]
            .iter()
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .collect::<Vec<_>>();
        if let codex::QuestionKind::Number(number) = &question.kind {
            if let Some(unit) = &number.unit {
                for value in &mut values {
                    let _ = write!(value, " {unit}");
                }
            }
        }
        values
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
}
impl Focusable for CodexView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
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
        let transcript = self.prepare_frame(window, cx);
        let follow = self.update_transcript_follow(&transcript, window, cx);
        let state = transcript.state.clone();
        let task_key = transcript.task_key;
        let task_cache = match &self.task_cache {
            Some(cache) if cache.key == task_key => cache.clone(),
            _ => {
                let cache = Rc::new(task_cache(&state.saved.items, task_key));
                self.task_cache = Some(cache.clone());
                cache
            }
        };
        let provider = state.saved.provider.label();
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
        let body = match self.render_transcript(&transcript, follow, window, cx) {
            TranscriptSurface::ReadOnly(preview) => return preview,
            TranscriptSurface::Interactive(body) => body,
        };
        let composer = ComposerFrame {
            placement: body.placement,
            input_locked: self.composer_input_locked(&state),
            glow_bounds: Rc::default(),
        };
        let mut surface = self
            .render_conversation(&state, body, &composer, window, cx)
            .child(self.render_composer_rail(
                &state,
                task_cache.list.as_ref(),
                &composer,
                window,
                cx,
            ));
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
fn active_turn_redirect_available(state: &codex::Snapshot) -> bool {
    state.can_redirect_active_turn()
}

const STEER_LABEL: &str = "Steer";
const STEER_DESCRIPTION: &str = "Steer into the current turn";

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
