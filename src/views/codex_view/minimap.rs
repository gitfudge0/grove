//! Transcript navigation rail, source excerpts, and cached scene.

#[cfg(test)]
use super::measured_slot;
use super::{
    action_key, c, codex, header_control_with_tooltip, rpx, tooltip_content, CodexView,
    PreparedItem, PreparedTranscript, Rc, MINIMAP_PREVIEW_W,
};
use crate::views::tokens::{CHROME_CONTROL_H, SPACE_20, SPACE_3XL, SPACE_LG, SPACE_SM, SPACE_XS};
use gpui::prelude::{
    FluentBuilder, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled,
};
use gpui::AppContext as _;
use gpui::{div, Context, Window};
#[cfg(test)]
use std::cell::Cell;

/// The minimap has a definite pane-sized host, so unchanged scenes can be replayed
/// independently of transcript animation and scroll frames.
pub(super) struct TranscriptMinimap {
    parent: gpui::WeakEntity<CodexView>,
    pub(super) prepared: Rc<PreparedTranscript>,
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
thread_local! { pub(super) static MINIMAP_BUILDS: Cell<usize> = const { Cell::new(0) }; }

/// A user input and the final (or latest streaming) assistant message before
/// the next input. Source indices stay distinct from rendered child indices.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct MessageMinimapEntry {
    pub(super) item_index: usize,
    pub(super) prompt: String,
    pub(super) response: String,
    prompt_preview: gpui::SharedString,
    response_preview: gpui::SharedString,
    label: gpui::SharedString,
    description: gpui::SharedString,
}

/// Assistant review metadata belongs to the review panel; retain raw saved items.
pub(super) fn transcript_item_text(item: &serde_json::Value) -> String {
    let text = codex::item_text(item);
    if item["type"] == "agentMessage" {
        grove_core::review_brief::display_text(&text)
    } else {
        text
    }
}

#[cfg(test)]
pub(super) fn message_minimap_entries<T: std::borrow::Borrow<serde_json::Value>>(
    items: &[T],
) -> Vec<MessageMinimapEntry> {
    message_minimap_entries_from(items, 0, None)
}

/// Preparation already extracted lossless display text. Borrow it instead of
/// repeatedly cloning large provider strings while deriving groups and excerpts.
pub(super) fn prepared_item_text<'a, T: std::borrow::Borrow<serde_json::Value>>(
    items: &'a [T],
    prepared: Option<&'a [Rc<PreparedItem>]>,
    index: usize,
) -> std::borrow::Cow<'a, str> {
    prepared.map_or_else(
        || std::borrow::Cow::Owned(transcript_item_text(items[index].borrow())),
        |prepared| std::borrow::Cow::Borrowed(prepared[index].text.as_ref()),
    )
}

pub(super) fn message_minimap_entries_from<T: std::borrow::Borrow<serde_json::Value>>(
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

pub(super) fn minimap_excerpt(text: &str) -> String {
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

pub(super) fn relative_message_time(timestamp: u64) -> String {
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

impl CodexView {
    pub(super) fn render_message_minimap(
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

    pub(super) fn build_message_minimap(
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
}
