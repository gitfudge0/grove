use gpui::{
    App, Axis, BorderStyle, Bounds, ContentMask, Edges, Element, ElementId, GlobalElementId,
    Hitbox, Hsla, InteractiveElement as _, IntoElement, IsZero as _, LayoutId, PaintQuad,
    ParentElement as _, Point, Position, ScrollHandle, ScrollWheelEvent,
    StatefulInteractiveElement as _, Style, StyleRefinement, Styled as _, Window, div, px,
    relative,
};
use gpui::{Corners, Pixels};

use crate::{AxisExt, StyledExt as _};

/// Delegates wheel input to [`ScrollableMask`] so vertical events keep bubbling, unlike gpui's native `overflow_x_scroll`.
pub(crate) fn horizontal_scroll_area(
    id: impl Into<ElementId>,
    scroll_handle: &ScrollHandle,
    style: &StyleRefinement,
    child: impl IntoElement,
) -> impl IntoElement {
    // The mask must be a sibling of the scrolled element, not a child, or it would slide away with the content as it scrolls.
    div()
        .w_full()
        .relative()
        .child(
            div()
                .id(id)
                .w_full()
                .refine_style(style)
                .overflow_hidden()
                .track_scroll(scroll_handle)
                .child(child),
        )
        .child(ScrollableMask::new(Axis::Horizontal, scroll_handle))
}

/// Consumes wheel events in the capture phase to win over ancestors like `gpui::list`; horizontal keeps consuming even at the edge (gpui bug #2468).
pub struct ScrollableMask {
    axis: Axis,
    scroll_handle: ScrollHandle,
    debug: Option<Hsla>,
}

impl ScrollableMask {
    pub fn new(axis: Axis, scroll_handle: &ScrollHandle) -> Self {
        Self {
            scroll_handle: scroll_handle.clone(),
            axis,
            debug: None,
        }
    }

    #[allow(dead_code)]
    pub fn debug(mut self) -> Self {
        self.debug = Some(gpui::yellow());
        self
    }
}

impl IntoElement for ScrollableMask {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for ScrollableMask {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.position = Position::Absolute;
        style.flex_grow = 1.0;
        style.flex_shrink = 1.0;
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();

        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
        let cover_bounds = Bounds {
            origin: Point {
                x: bounds.origin.x,
                y: bounds.origin.y - bounds.size.height,
            },
            size: bounds.size,
        };

        window.insert_hitbox(cover_bounds, gpui::HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        _: &mut App,
    ) {
        let is_horizontal = self.axis.is_horizontal();
        let line_height = window.line_height();
        let bounds = hitbox.bounds;

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if let Some(color) = self.debug {
                window.paint_quad(PaintQuad {
                    bounds,
                    border_widths: Edges::all(px(1.0)),
                    border_color: color,
                    background: gpui::transparent_white().into(),
                    corner_radii: Corners::all(px(0.)),
                    border_style: BorderStyle::default(),
                });
            }

            window.on_mouse_event({
                let view_id = window.current_view();
                let scroll_handle = self.scroll_handle.clone();
                let hitbox_id = hitbox.id;

                move |event: &ScrollWheelEvent, phase, window, cx| {
                    // Capture phase, since `gpui::list` registers listeners after children and would consume first in bubble phase.
                    if !(phase.capture() && hitbox_id.should_handle_scroll(window)) {
                        return;
                    }

                    let mut offset = scroll_handle.offset();
                    let mut delta = event.delta.pixel_delta(line_height);

                    // Only one axis scrolls at a time; a trackpad can deliver both x and y, so keep the larger.
                    if !delta.x.is_zero() && !delta.y.is_zero() {
                        if delta.x.abs() > delta.y.abs() {
                            delta.y = px(0.);
                        } else {
                            delta.x = px(0.);
                        }
                    }

                    if !is_horizontal {
                        // Must clamp here too, or a bubbled unclamped offset reads as "room to scroll".
                        let axis_max = scroll_handle.max_offset().y.max(px(0.));
                        let current = offset.y.clamp(-axis_max, px(0.));
                        let new_offset = (current + delta.y).clamp(-axis_max, px(0.));
                        if new_offset == current {
                            return;
                        }

                        offset.y = new_offset;
                        scroll_handle.set_offset(offset);
                        cx.notify(view_id);
                        cx.stop_propagation();
                        return;
                    }

                    offset.x += delta.x;

                    // `set_offset` doesn't clamp, so even at the edge this consumes rather than bubbling.
                    if offset != scroll_handle.offset() {
                        scroll_handle.set_offset(offset);
                        cx.notify(view_id);
                        cx.stop_propagation();
                    }
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        Context, IntoElement, ListAlignment, ListState, Render, ScrollDelta, ScrollWheelEvent,
        TestAppContext, VisualTestContext, Window, div, list, point, px,
    };

    struct HorizontalScrollAreaTest {
        scroll_handle: ScrollHandle,
    }

    impl Render for HorizontalScrollAreaTest {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(100.)).h(px(40.)).child(horizontal_scroll_area(
                "horizontal-scroll-area",
                &self.scroll_handle,
                &Default::default(),
                div().w(px(300.)).h(px(40.)),
            ))
        }
    }

    #[gpui::test]
    fn horizontal_scroll_area_ignores_vertical_wheel(cx: &mut TestAppContext) {
        let scroll_handle = ScrollHandle::new();
        let (_, cx) = cx.add_window_view({
            let scroll_handle = scroll_handle.clone();
            move |_, _| HorizontalScrollAreaTest {
                scroll_handle: scroll_handle.clone(),
            }
        });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(10.), px(10.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
            ..Default::default()
        });

        assert_eq!(scroll_handle.offset().x, px(0.));
    }

    /// Reproduces the markdown table case: `gpui::list` would consume `delta.y` first in bubble phase.
    struct ListWithHorizontalAreaTest {
        scroll_handle: ScrollHandle,
        list_state: ListState,
        occluded: bool,
    }

    impl Render for ListWithHorizontalAreaTest {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let scroll_handle = self.scroll_handle.clone();
            let mut root = div().w(px(100.)).h(px(100.)).child(
                list(self.list_state.clone(), move |ix, _, _| {
                    if ix == 0 {
                        horizontal_scroll_area(
                            "horizontal-scroll-area",
                            &scroll_handle,
                            &Default::default(),
                            div().w(px(300.)).h(px(40.)),
                        )
                        .into_any_element()
                    } else {
                        div().w(px(100.)).h(px(40.)).into_any_element()
                    }
                })
                .w_full()
                .h_full(),
            );
            if self.occluded {
                root = root.child(div().absolute().top_0().left_0().size_full().occlude());
            }
            root
        }
    }

    fn setup_list_test<'a>(
        cx: &'a mut TestAppContext,
        scroll_handle: &ScrollHandle,
        list_state: &ListState,
        occluded: bool,
    ) -> &'a mut VisualTestContext {
        let (_, cx) = cx.add_window_view({
            let scroll_handle = scroll_handle.clone();
            let list_state = list_state.clone();
            move |_, _| ListWithHorizontalAreaTest {
                scroll_handle: scroll_handle.clone(),
                list_state: list_state.clone(),
                occluded,
            }
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        cx
    }

    #[gpui::test]
    fn horizontal_scroll_area_in_list_keeps_horizontal_dominant_wheel(cx: &mut TestAppContext) {
        let scroll_handle = ScrollHandle::new();
        let list_state = ListState::new(10, ListAlignment::Top, px(0.));
        let cx = setup_list_test(cx, &scroll_handle, &list_state, false);

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(10.), px(10.)),
            delta: ScrollDelta::Pixels(point(px(-40.), px(-10.))),
            ..Default::default()
        });

        assert_eq!(scroll_handle.offset().x, px(-40.));
        let scroll_top = list_state.logical_scroll_top();
        assert_eq!((scroll_top.item_ix, scroll_top.offset_in_item), (0, px(0.)));
    }

    #[gpui::test]
    fn horizontal_scroll_area_in_list_bubbles_vertical_dominant_wheel(cx: &mut TestAppContext) {
        let scroll_handle = ScrollHandle::new();
        let list_state = ListState::new(10, ListAlignment::Top, px(0.));
        let cx = setup_list_test(cx, &scroll_handle, &list_state, false);

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(10.), px(10.)),
            delta: ScrollDelta::Pixels(point(px(-10.), px(-40.))),
            ..Default::default()
        });

        assert_eq!(scroll_handle.offset().x, px(0.));
        let scroll_top = list_state.logical_scroll_top();
        assert_ne!((scroll_top.item_ix, scroll_top.offset_in_item), (0, px(0.)));
    }

    #[gpui::test]
    fn horizontal_scroll_area_covers_viewport_after_scrolled(cx: &mut TestAppContext) {
        let scroll_handle = ScrollHandle::new();
        let list_state = ListState::new(10, ListAlignment::Top, px(0.));
        let cx = setup_list_test(cx, &scroll_handle, &list_state, false);

        scroll_handle.set_offset(point(px(-150.), px(0.)));
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });

        // The mask must still cover the viewport, not slide away with the scrolled content.
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(90.), px(10.)),
            delta: ScrollDelta::Pixels(point(px(-40.), px(-10.))),
            ..Default::default()
        });

        assert_eq!(scroll_handle.offset().x, px(-190.));
        let scroll_top = list_state.logical_scroll_top();
        assert_eq!((scroll_top.item_ix, scroll_top.offset_in_item), (0, px(0.)));
    }

    #[gpui::test]
    fn horizontal_scroll_area_traps_wheel_at_edge(cx: &mut TestAppContext) {
        let scroll_handle = ScrollHandle::new();
        let list_state = ListState::new(10, ListAlignment::Top, px(0.));
        let cx = setup_list_test(cx, &scroll_handle, &list_state, false);

        scroll_handle.set_offset(point(px(-200.), px(0.)));
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(10.), px(10.)),
            delta: ScrollDelta::Pixels(point(px(-40.), px(-10.))),
            ..Default::default()
        });

        let scroll_top = list_state.logical_scroll_top();
        assert_eq!((scroll_top.item_ix, scroll_top.offset_in_item), (0, px(0.)));
    }

    #[gpui::test]
    fn horizontal_scroll_area_ignores_wheel_when_occluded(cx: &mut TestAppContext) {
        let scroll_handle = ScrollHandle::new();
        let list_state = ListState::new(10, ListAlignment::Top, px(0.));
        let cx = setup_list_test(cx, &scroll_handle, &list_state, true);

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(10.), px(10.)),
            delta: ScrollDelta::Pixels(point(px(-40.), px(-10.))),
            ..Default::default()
        });

        assert_eq!(scroll_handle.offset().x, px(0.));
    }

    #[gpui::test]
    fn horizontal_scroll_area_uses_horizontal_wheel(cx: &mut TestAppContext) {
        let scroll_handle = ScrollHandle::new();
        let (_, cx) = cx.add_window_view({
            let scroll_handle = scroll_handle.clone();
            move |_, _| HorizontalScrollAreaTest {
                scroll_handle: scroll_handle.clone(),
            }
        });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(10.), px(10.)),
            delta: ScrollDelta::Pixels(point(px(-40.), px(0.))),
            ..Default::default()
        });

        assert_eq!(scroll_handle.offset().x, px(-40.));
    }

    /// Reproduces the DataTable case: a scrollable element nested inside an outer vertical scroller.
    struct NestedVerticalScrollTest {
        outer_handle: ScrollHandle,
        inner_handle: ScrollHandle,
        inner_content_height: Pixels,
    }

    impl Render for NestedVerticalScrollTest {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("outer")
                .w(px(100.))
                .h(px(100.))
                .overflow_y_scroll()
                .track_scroll(&self.outer_handle)
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(60.))
                        .child(
                            div()
                                .id("inner")
                                .size_full()
                                .overflow_y_scroll()
                                .track_scroll(&self.inner_handle)
                                .child(div().w_full().h(self.inner_content_height)),
                        )
                        .child(ScrollableMask::new(Axis::Vertical, &self.inner_handle)),
                )
                .child(div().w_full().h(px(400.)))
        }
    }

    fn setup_nested_vertical_test<'a>(
        cx: &'a mut TestAppContext,
        outer_handle: &ScrollHandle,
        inner_handle: &ScrollHandle,
        inner_content_height: Pixels,
    ) -> &'a mut VisualTestContext {
        let (_, cx) = cx.add_window_view({
            let outer_handle = outer_handle.clone();
            let inner_handle = inner_handle.clone();
            move |_, _| NestedVerticalScrollTest {
                outer_handle: outer_handle.clone(),
                inner_handle: inner_handle.clone(),
                inner_content_height,
            }
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        cx
    }

    #[gpui::test]
    fn vertical_mask_consumes_wheel_when_scrollable(cx: &mut TestAppContext) {
        let outer_handle = ScrollHandle::new();
        let inner_handle = ScrollHandle::new();
        let cx = setup_nested_vertical_test(cx, &outer_handle, &inner_handle, px(300.));

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(10.), px(10.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
            ..Default::default()
        });

        assert_eq!(inner_handle.offset().y, px(-40.));
        assert_eq!(outer_handle.offset().y, px(0.));
    }

    #[gpui::test]
    fn vertical_mask_hands_off_to_parent_at_edge(cx: &mut TestAppContext) {
        let outer_handle = ScrollHandle::new();
        let inner_handle = ScrollHandle::new();
        let cx = setup_nested_vertical_test(cx, &outer_handle, &inner_handle, px(300.));

        inner_handle.set_offset(point(px(0.), px(-240.)));
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(10.), px(10.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
            ..Default::default()
        });

        assert_eq!(outer_handle.offset().y, px(-40.));
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert_eq!(inner_handle.offset().y, px(-240.));
    }

    #[gpui::test]
    fn vertical_mask_bubbles_when_no_overflow(cx: &mut TestAppContext) {
        let outer_handle = ScrollHandle::new();
        let inner_handle = ScrollHandle::new();
        let cx = setup_nested_vertical_test(cx, &outer_handle, &inner_handle, px(40.));

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(10.), px(10.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
            ..Default::default()
        });

        assert_eq!(outer_handle.offset().y, px(-40.));
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert_eq!(inner_handle.offset().y, px(0.));
    }

    #[gpui::test]
    fn vertical_mask_ignores_transient_overscroll(cx: &mut TestAppContext) {
        let outer_handle = ScrollHandle::new();
        let inner_handle = ScrollHandle::new();
        let cx = setup_nested_vertical_test(cx, &outer_handle, &inner_handle, px(300.));

        inner_handle.set_offset(point(px(0.), px(-240.)));
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });

        // Two events with no redraw between: the first leaves the offset unclamped past the edge, which must not swallow the second.
        for _ in 0..2 {
            cx.simulate_event(ScrollWheelEvent {
                position: point(px(10.), px(10.)),
                delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
                ..Default::default()
            });
        }

        assert_eq!(outer_handle.offset().y, px(-80.));
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert_eq!(inner_handle.offset().y, px(-240.));
    }

    /// Only a capture-phase mask can stop `gpui::list` scrolling on the same event.
    struct ListWithVerticalAreaTest {
        scroll_handle: ScrollHandle,
        list_state: ListState,
    }

    impl Render for ListWithVerticalAreaTest {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let scroll_handle = self.scroll_handle.clone();
            div().w(px(100.)).h(px(100.)).child(
                list(self.list_state.clone(), move |ix, _, _| {
                    if ix == 0 {
                        div()
                            .relative()
                            .w_full()
                            .h(px(60.))
                            .child(
                                div()
                                    .id("inner")
                                    .size_full()
                                    .overflow_y_scroll()
                                    .track_scroll(&scroll_handle)
                                    .child(div().w_full().h(px(300.))),
                            )
                            .child(ScrollableMask::new(Axis::Vertical, &scroll_handle))
                            .into_any_element()
                    } else {
                        div().w(px(100.)).h(px(40.)).into_any_element()
                    }
                })
                .w_full()
                .h_full(),
            )
        }
    }

    #[gpui::test]
    fn vertical_mask_in_list_consumes_wheel_when_scrollable(cx: &mut TestAppContext) {
        let scroll_handle = ScrollHandle::new();
        let list_state = ListState::new(10, ListAlignment::Top, px(0.));
        let (_, cx) = cx.add_window_view({
            let scroll_handle = scroll_handle.clone();
            let list_state = list_state.clone();
            move |_, _| ListWithVerticalAreaTest {
                scroll_handle: scroll_handle.clone(),
                list_state: list_state.clone(),
            }
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(10.), px(10.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
            ..Default::default()
        });

        assert_eq!(scroll_handle.offset().y, px(-40.));
        let scroll_top = list_state.logical_scroll_top();
        assert_eq!((scroll_top.item_ix, scroll_top.offset_in_item), (0, px(0.)));
    }
}
