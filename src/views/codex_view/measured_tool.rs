//! Retain exact collapsed tool geometry and its original keyboard destination.
//! Expanded/raw details and active tools keep the ordinary eager renderer.
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
    cache: Rc<RefCell<Measurement>>,
}
pub(super) struct ToolLayout {
    element: AnyElement,
    cached_size: Option<Size<Pixels>>,
    real: bool,
    clip: Option<Bounds<Pixels>>,
}
impl MeasuredTool {
    pub fn new(
        view: WeakEntity<CodexView>,
        state: Rc<codex::Snapshot>,
        index: usize,
        key: String,
        focus: FocusHandle,
        cache: Rc<RefCell<Measurement>>,
    ) -> Self {
        Self {
            view,
            state,
            index,
            key,
            focus,
            cache,
        }
    }
    fn materialize(&self, window: &mut Window, cx: &mut App) -> AnyElement {
        self.cache.borrow_mut().label =
            tool_activity_summary(self.state.saved.items[self.index].as_ref(), false);
        self.view
            .update(cx, |view, cx| {
                view.render_tool_row(self.index, &self.state, false, &self.focus, window, cx)
            })
            .unwrap_or_else(|_| div().into_any_element())
    }
    fn proxy(&self, size: Size<Pixels>) -> AnyElement {
        let view = self.view.clone();
        let key = self.key.clone();
        div()
            .id(("codex-tool-proxy", self.index))
            .w_full()
            .h(size.height)
            .flex_shrink_0()
            .track_focus(&self.focus)
            .tab_index(0)
            .role(gpui::Role::Button)
            .aria_label(self.cache.borrow().label.clone())
            .aria_expanded(false)
            .on_key_down(move |event, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") && !event.is_held {
                    let _ = view.update(cx, |view, cx| view.toggle_expansion(&key, cx));
                    cx.stop_propagation();
                }
            })
            .into_any_element()
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
                && !self.focus.contains_focused(window, cx)
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
        (
            id,
            ToolLayout {
                element,
                cached_size: size,
                real: size.is_none(),
                clip: None,
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
        if layout.real && !width_changed {
            let mut cache = self.cache.borrow_mut();
            if !cache.pending {
                cache.size = Some(bounds.size);
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
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(
            layout.clip.map(|bounds| gpui::ContentMask { bounds }),
            |window| layout.element.paint(window, cx),
        );
    }
}
