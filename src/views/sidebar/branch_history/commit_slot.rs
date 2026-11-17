//! Fixed-height commit rows preserve their logical keyboard destinations while
//! deferring graph and subject construction outside the list viewport.
use super::*;
use gpui::{
    App, Bounds, Element, ElementId, FocusHandle, GlobalElementId, InspectorElementId, LayoutId,
    Pixels, WeakEntity, Window,
};
use std::rc::Rc;

pub(super) struct CommitSlot {
    owner: WeakEntity<Sidebar>,
    state: Rc<BranchHistoryState>,
    index: usize,
    graph_width: f32,
    focus: FocusHandle,
    interactivity: gpui::Interactivity,
}
impl CommitSlot {
    pub(super) fn new(
        owner: WeakEntity<Sidebar>,
        state: Rc<BranchHistoryState>,
        index: usize,
        graph_width: f32,
        focus: FocusHandle,
    ) -> Self {
        let target = owner.clone();
        let id = state
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.commits[index].id.clone())
            .unwrap_or_default();
        let mut proxy =
            div()
                .track_focus(&focus)
                .tab_index(0)
                .on_key_down(move |event, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") && !event.is_held {
                        let _ = target.update(cx, |sidebar, cx| {
                            sidebar.act(Action::SelectCommit(id.clone()), window, cx);
                        });
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                });
        Self {
            owner,
            state,
            index,
            graph_width,
            focus,
            interactivity: std::mem::take(proxy.interactivity()),
        }
    }
}
impl IntoElement for CommitSlot {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for CommitSlot {
    type RequestLayoutState = ();
    type PrepaintState = Option<AnyElement>;
    fn id(&self) -> Option<ElementId> {
        Some(("history-commit-slot", self.index).into())
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
        style.size.height = rpx(COMMIT_ROW_H).into();
        style.min_size.height = rpx(COMMIT_ROW_H).into();
        style.min_size.width = rpx(self.graph_width + ROW_CONTENT_MIN_W).into();
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
        let selected = self.state.snapshot.as_ref().is_some_and(|snapshot| {
            self.state.selected.as_ref() == Some(&snapshot.commits[self.index].id)
        });
        if bounds.intersects(&window.content_mask().bounds)
            || self.focus.contains_focused(window, cx)
            || selected
            || window.is_a11y_active()
            || window.is_inspector_picking(cx)
        {
            let mut row = self
                .owner
                .update(cx, |owner, cx| {
                    owner.render_history_commit(
                        &self.state,
                        self.index,
                        self.graph_width,
                        &self.focus,
                        window,
                        cx,
                    )
                })
                .unwrap_or_else(|_| div().into_any_element());
            row.layout_as_root(bounds.size.into(), window, cx);
            row.prepaint_at(bounds.origin, window, cx);
            Some(row)
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
        row: &mut Option<AnyElement>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(row) = row {
            row.paint(window, cx);
        } else {
            self.interactivity
                .paint(None, None, bounds, None, window, cx, |_, _, _| {});
        }
    }
}
