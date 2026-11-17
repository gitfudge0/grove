//! Retain exact tool geometry and original keyboard destinations offscreen.
//! Active tools, selection and accessibility keep the ordinary eager renderer.
use super::*;
use gpui::{
    AnyElement, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, LayoutId, Pixels,
    Size, TextStyle, WeakEntity,
};
use std::cell::RefCell;

#[derive(Default)]
pub(super) struct Measurement {
    source: Option<codex::TranscriptItem>,
    size: Option<Size<Pixels>>,
    text_style: Option<TextStyle>,
    rem: Pixels,
    viewport_width: Pixels,
    pending: bool,
    label: String,
    expanded: bool,
    copy_focus: Option<FocusHandle>,
}
#[cfg(test)]
thread_local! {
    pub(super) static MATERIALIZED: Cell<usize> = const { Cell::new(0) };
    pub(super) static FORCE_EAGER: Cell<bool> = const { Cell::new(false) };
}
pub(super) fn force_eager() -> bool {
    #[cfg(test)]
    {
        measured_slot::force_eager()
            || FORCE_EAGER.with(Cell::get)
            || std::env::var_os("GROVE_PERF_EAGER_TOOLS").is_some()
    }
    #[cfg(not(test))]
    {
        false
    }
}
pub(super) struct MeasuredTool {
    view: WeakEntity<CodexView>,
    state: Rc<codex::Snapshot>,
    index: usize,
    key: String,
    focus: FocusHandle,
    expanded: bool,
    cache: Rc<RefCell<Measurement>>,
}
pub(super) struct ToolLayout {
    element: AnyElement,
    cached_size: Option<Size<Pixels>>,
    real: bool,
    clip: Option<Bounds<Pixels>>,
    controls: Vec<AnyElement>,
}
impl MeasuredTool {
    pub fn new(
        view: WeakEntity<CodexView>,
        state: Rc<codex::Snapshot>,
        index: usize,
        key: String,
        focus: FocusHandle,
        expanded: bool,
        cache: Rc<RefCell<Measurement>>,
    ) -> Self {
        Self {
            view,
            state,
            index,
            key,
            focus,
            expanded,
            cache,
        }
    }
    fn materialize(&self, window: &mut Window, cx: &mut App) -> AnyElement {
        self.cache.borrow_mut().label =
            tool_activity_summary(self.state.saved.items[self.index].as_ref(), false);
        self.view
            .update(cx, |view, cx| {
                let row =
                    view.render_tool_row(self.index, &self.state, false, &self.focus, window, cx);
                self.cache.borrow_mut().copy_focus = self
                    .expanded
                    .then(|| {
                        view.message_focus
                            .get(&format!(
                                "tool-copy:{}:{}:{}",
                                self.state.saved.thread_id.as_deref().unwrap_or_default(),
                                self.index,
                                self.key,
                            ))
                            .cloned()
                    })
                    .flatten();
                row
            })
            .unwrap_or_else(|_| div().into_any_element())
    }
    fn proxy(&self, size: Size<Pixels>) -> AnyElement {
        div()
            .id(("codex-tool-proxy", self.index))
            .w_full()
            .h(size.height)
            .flex_shrink_0()
            .into_any_element()
    }
    fn control_aliases(
        &self,
        layout: LayoutId,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        let view = self.view.clone();
        let key = self.key.clone();
        let control = div()
            .id(("codex-tool-disclosure-alias", self.index))
            .track_focus(&self.focus)
            .tab_index(0)
            .role(gpui::Role::Button)
            .aria_label(self.cache.borrow().label.clone())
            .aria_expanded(self.expanded)
            .on_key_down(move |event, _, cx| {
                if action_key(event) {
                    let _ = view.update(cx, |view, cx| view.toggle_expansion(&key, cx));
                    cx.stop_propagation();
                }
            });
        let mut aliases = vec![measured_slot::control_alias(
            layout,
            self.focus.clone(),
            control,
        )];
        if let Some(focus) = self
            .cache
            .borrow()
            .copy_focus
            .clone()
            .filter(|_| self.expanded)
        {
            let view = self.view.clone();
            let state = self.state.clone();
            let index = self.index;
            let control = div()
                .id(("codex-tool-copy-alias", self.index))
                .track_focus(&focus)
                .tab_index(0)
                .aria_label("Copy tool call")
                .on_key_down(move |event, _, cx| {
                    if action_key(event) {
                        let _ = view.update(cx, |view, cx| {
                            let text = view.prepared_item(&state, index).detail();
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                text.to_string(),
                            ));
                        });
                        cx.stop_propagation();
                    }
                });
            aliases.push(measured_slot::control_alias(layout, focus, control));
        }
        for alias in &mut aliases {
            alias.request_layout(window, cx);
        }
        aliases
    }
    fn correct(&self, window: &mut Window) {
        let mut cache = self.cache.borrow_mut();
        cache.size = None;
        if std::mem::replace(&mut cache.pending, true) {
            return;
        }
        let cache = self.cache.clone();
        let view = self.view.clone();
        window.on_next_frame(move |_, cx| {
            cache.borrow_mut().pending = false;
            let _ = view.update(cx, |_, cx| cx.notify());
        });
    }
}
impl IntoElement for MeasuredTool {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for MeasuredTool {
    type RequestLayoutState = ToolLayout;
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ToolLayout) {
        let cache = self.cache.borrow();
        let size = cache.size.filter(|_| {
            !cache.pending
                && cache.expanded == self.expanded
                && (!self.expanded || cache.copy_focus.is_some())
                && !self.focus.contains_focused(window, cx)
                && !cache
                    .copy_focus
                    .as_ref()
                    .is_some_and(|focus| focus.contains_focused(window, cx))
                && cache.rem == window.rem_size()
                && cache.viewport_width == window.viewport_size().width
                && cache.text_style.as_ref() == Some(&window.text_style())
                && cache.source.as_ref().is_some_and(|old| {
                    std::ptr::eq::<serde_json::Value>(
                        old.as_ref(),
                        self.state.saved.items[self.index].as_ref(),
                    )
                })
        });
        drop(cache);
        let mut element =
            size.map_or_else(|| self.materialize(window, cx), |size| self.proxy(size));
        let id = element.request_layout(window, cx);
        let controls = if size.is_some() {
            self.control_aliases(id, window, cx)
        } else {
            Vec::new()
        };
        (
            id,
            ToolLayout {
                element,
                cached_size: size,
                real: size.is_none(),
                clip: None,
                controls,
            },
        )
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut ToolLayout,
        window: &mut Window,
        cx: &mut App,
    ) {
        let width_changed = layout
            .cached_size
            .is_some_and(|old| old.width != bounds.size.width);
        if !layout.real && (bounds.intersects(&window.content_mask().bounds) || width_changed) {
            layout.element = self.materialize(window, cx);
            layout
                .element
                .layout_as_root(bounds.size.into(), window, cx);
            layout.clip = width_changed.then_some(bounds);
            window.with_content_mask(
                layout.clip.map(|bounds| gpui::ContentMask { bounds }),
                |window| layout.element.prepaint_at(bounds.origin, window, cx),
            );
            layout.real = true;
            if width_changed {
                self.correct(window);
            }
        } else {
            layout.element.prepaint(window, cx);
        }
        if !layout.real {
            for control in &mut layout.controls {
                control.prepaint_at(bounds.origin, window, cx);
            }
        }
        if layout.real && !width_changed {
            let mut cache = self.cache.borrow_mut();
            if !cache.pending {
                cache.size = Some(bounds.size);
                cache.expanded = self.expanded;
                cache.source = Some(self.state.saved.items[self.index].clone());
                cache.text_style = Some(window.text_style());
                cache.rem = window.rem_size();
                cache.viewport_width = window.viewport_size().width;
            }
        }
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        layout: &mut ToolLayout,
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(
            layout.clip.map(|bounds| gpui::ContentMask { bounds }),
            |window| layout.element.paint(window, cx),
        );
        if !layout.real {
            for control in &mut layout.controls {
                control.paint(window, cx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{draw, fixture, snapshot};
    use super::*;
    use serde_json::json;

    fn settle(cx: &mut gpui::VisualTestContext) {
        for _ in 0..4 {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(16));
            cx.update(gpui::Window::simulate_next_frame);
            draw(cx);
        }
    }

    #[gpui::test]
    fn expanded_offscreen_tools_reuse_geometry_and_keep_original_copy_focus(
        cx: &mut gpui::TestAppContext,
    ) {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let (view, cx, _, _) = fixture(cx);
            cx.update(|_, cx| {
                cx.set_reduce_motion(true);
                view.update(cx, |view, cx| {
                    view.expanded.insert("work-0".into());
                    view.expanded.insert("tool-command".into());
                    view.session.update(cx, |session, cx| {
                        let mut state = snapshot();
                        state.saved.provider = provider;
                        state.saved.thread_id = Some("expanded-tool-test".into());
                        state.saved.items = vec![
                            json!({"id":"user", "type":"userMessage", "text":"Inspect tool details"}),
                            json!({"id":"command", "type":"commandExecution", "command":"printf preserved", "status":"completed", "aggregatedOutput":"Original full tool output"}),
                            json!({"id":"answer", "type":"agentMessage", "text":"Tool complete", "phase":"final_answer", "groveCompleted":true}),
                        ].into();
                        for index in 0..30 {
                            state.saved.items.push(json!({"id":format!("user-{index}"), "type":"userMessage", "text":"Later prompt"}));
                            state.saved.items.push(json!({"id":format!("answer-{index}"), "type":"agentMessage", "text":"Later answer\n\nPreserved paragraph.", "phase":"final_answer", "groveCompleted":true}));
                        }
                        session.state = state.into();
                        cx.notify();
                    });
                    cx.notify();
                });
            });
            draw(cx);
            cx.update(|window, cx| {
                view.update(cx, |view, cx| view.jump_to_transcript_item(0, window, cx));
            });
            settle(cx);
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.smooth_follow = false;
                    view.scroll.scroll_to_bottom();
                    cx.notify();
                });
            });
            settle(cx);
            let original_max = view.read_with(cx, |view, _| view.scroll.max_offset());
            MATERIALIZED.with(|count| count.set(0));
            cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
            settle(cx);
            assert_eq!(
                MATERIALIZED.with(Cell::get),
                0,
                "unchanged expanded offscreen tools must reuse their extent"
            );
            let copy_key = "tool-copy:expanded-tool-test:1:tool-command";
            let (copy_focus, expected) = view.read_with(cx, |view, cx| {
                (
                    view.message_focus[copy_key].clone(),
                    tool_detail(&view.session.read(cx).state.saved.items[1]),
                )
            });
            cx.update(|window, cx| copy_focus.focus(window, cx));
            settle(cx);
            assert!(
                cx.debug_bounds("codex-copy-tool").is_some(),
                "focused Copy must materialize its original control"
            );
            cx.simulate_keystrokes("enter");
            settle(cx);
            assert_eq!(
                cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
                Some(expected)
            );
            assert_eq!(
                view.read_with(cx, |view, _| view.scroll.max_offset()),
                original_max
            );
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    window.blur();
                    view.toggle_expansion("tool-command", cx);
                });
            });
            settle(cx);
            assert!(!view.read_with(cx, |view, _| view.expanded.contains("tool-command")));
            assert!(
                view.read_with(cx, |view, _| view.scroll.max_offset().y) < original_max.y,
                "collapsing a projected expanded row must invalidate its taller measurement"
            );
        }
    }
}
