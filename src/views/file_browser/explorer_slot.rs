//! Fixed-height explorer rows keep their original Tab destinations while
//! materializing glyphs, labels and pointer controls only inside the viewport.
use super::*;
use gpui::{
    Bounds, Element, ElementId, GlobalElementId, InspectorElementId, LayoutId, Pixels, WeakEntity,
};

pub(super) struct ExplorerSlot {
    owner: WeakEntity<FileBrowser>,
    entry: ExplorerEntry,
    focus: FocusHandle,
    interactivity: gpui::Interactivity,
}
impl ExplorerSlot {
    pub(super) fn new(
        owner: WeakEntity<FileBrowser>,
        entry: ExplorerEntry,
        focus: FocusHandle,
    ) -> Self {
        let target = owner.clone();
        let activation = entry.clone();
        let mut proxy =
            div()
                .track_focus(&focus)
                .tab_index(0)
                .on_key_down(move |event, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") && !event.is_held {
                        let _ = target.update(cx, |browser, cx| {
                            if activation.directory {
                                browser.toggle_directory(activation.path.clone(), cx);
                            } else {
                                browser.open(
                                    FileTarget {
                                        path: activation.path.clone(),
                                        line: None,
                                        transcript_link: false,
                                    },
                                    window,
                                    cx,
                                );
                            }
                        });
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                });
        Self {
            owner,
            entry,
            focus,
            interactivity: std::mem::take(proxy.interactivity()),
        }
    }
    fn materialize(&self, _window: &mut Window, cx: &mut App) -> AnyElement {
        let entry = self.entry.clone();
        self.owner
            .update(cx, |owner, cx| owner.render_explorer_row(&entry, cx))
            .unwrap_or_else(|_| div().into_any_element())
    }
}
impl IntoElement for ExplorerSlot {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for ExplorerSlot {
    type RequestLayoutState = ();
    type PrepaintState = Option<AnyElement>;
    fn id(&self) -> Option<ElementId> {
        Some(format!("explorer-slot-{}", self.entry.path.display()).into())
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
    ) -> (LayoutId, ()) {
        let mut style = gpui::Style::default();
        style.size.width = gpui::relative(1.).into();
        style.size.height = rpx(CONTROL_H).into();
        style.min_size.height = rpx(CONTROL_H).into();
        style.flex_shrink = 0.;
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        if bounds.intersects(&window.content_mask().bounds)
            || self.focus.contains_focused(window, cx)
            || window.is_a11y_active()
            || window.is_inspector_picking(cx)
        {
            let mut element = self.materialize(window, cx);
            element.layout_as_root(bounds.size.into(), window, cx);
            element.prepaint_at(bounds.origin, window, cx);
            Some(element)
        } else {
            window.set_focus_handle(&self.focus, cx);
            None
        }
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (): &mut (),
        element: &mut Option<AnyElement>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(element) = element {
            element.paint(window, cx);
        } else {
            self.interactivity
                .paint(None, None, bounds, None, window, cx, |_, _, _| {});
        }
    }
}
