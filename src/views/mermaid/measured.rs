//! Keep ready, unfocused diagrams mounted without rebuilding offscreen controls.
use super::*;
use gpui::{Bounds, Element, ElementId, GlobalElementId, InspectorElementId, LayoutId, TextStyle};
use std::{cell::RefCell, rc::Rc};

pub(super) type Cache = Rc<RefCell<Option<Rc<Measurement>>>>;
pub(super) struct Measurement {
    source: String,
    image: Arc<Image>,
    ratio: f32,
    image_width: f32,
    viewport: Size<Pixels>,
    size: Size<Pixels>,
    rem: Pixels,
    window_width: Pixels,
    theme: u64,
    style: TextStyle,
}
pub(super) struct DiagramElement(pub gpui::Entity<DiagramViewer>);
pub(super) struct Layout {
    element: Option<AnyElement>,
    cached: Option<Rc<Measurement>>,
    viewport: Size<Pixels>,
    layout: LayoutId,
    proxies: Vec<AnyElement>,
}
#[cfg(test)]
thread_local! { pub(super) static MATERIALIZED: std::cell::Cell<usize> = const {std::cell::Cell::new(0)}; }
fn forced() -> bool {
    #[cfg(test)]
    {
        std::env::var_os("GROVE_PERF_EAGER_DIAGRAMS").is_some()
    }
    #[cfg(not(test))]
    {
        false
    }
}
impl IntoElement for DiagramElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for DiagramElement {
    type RequestLayoutState = Layout;
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
    ) -> (LayoutId, Layout) {
        let view = self.0.read(cx);
        let viewport = view.viewport;
        let eligible = !forced()
            && !view.fullscreen
            && view.zoom == 1.
            && view.drag.is_none()
            && !window.is_a11y_active()
            && !window.is_inspector_picking(cx)
            && !gpui_component::text::has_active_text_selection(window, cx)
            && !view.focus.contains_focused(window, cx)
            && !view
                .controls
                .iter()
                .any(|focus| focus.contains_focused(window, cx));
        let cached = view
            .measurement
            .borrow()
            .as_ref()
            .filter(|m| {
                eligible
                    && m.source == view.source
                    && Arc::ptr_eq(&m.image, &view.image)
                    && m.ratio == view.ratio
                    && m.image_width == view.width
                    && m.viewport == viewport
                    && m.rem == window.rem_size()
                    && m.window_width == window.viewport_size().width
                    && m.theme == grove_core::theme::generation()
                    && m.style == window.text_style()
            })
            .cloned();
        let (layout, element) = if let Some(m) = &cached {
            let size = m.size;
            let mut style = gpui::Style::default();
            style.size.width = gpui::relative(1.).into();
            (
                window.request_measured_layout(style, move |known, available, _, _| {
                    let width = known.width.unwrap_or_else(|| match available.width {
                        gpui::AvailableSpace::Definite(width) => size.width.min(width),
                        _ => size.width,
                    });
                    gpui::size(width, known.height.unwrap_or(size.height))
                }),
                None,
            )
        } else {
            #[cfg(test)]
            MATERIALIZED.with(|n| n.set(n.get() + 1));
            let mut element = self.0.clone().into_any_element();
            let layout = element.request_layout(window, cx);
            (layout, Some(element))
        };
        (
            layout,
            Layout {
                element,
                cached,
                viewport,
                layout,
                proxies: Vec::new(),
            },
        )
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Layout,
        window: &mut Window,
        cx: &mut App,
    ) {
        let width_changed = layout
            .cached
            .as_ref()
            .is_some_and(|m| m.size.width != bounds.size.width);
        if layout.element.is_none()
            && (bounds.intersects(&window.content_mask().bounds) || width_changed)
        {
            #[cfg(test)]
            MATERIALIZED.with(|n| n.set(n.get() + 1));
            let mut element = self.0.clone().into_any_element();
            element.layout_as_root(bounds.size.into(), window, cx);
            element.prepaint_at(bounds.origin, window, cx);
            layout.element = Some(element);
            if width_changed {
                *self.0.read(cx).measurement.borrow_mut() = None;
                let owner = self.0.downgrade();
                window.on_next_frame(move |_, cx| {
                    let _ = owner.update(cx, |_, cx| cx.notify());
                });
            }
        } else if let Some(element) = &mut layout.element {
            element.prepaint(window, cx);
        }
        if layout.element.is_some() {
            let view = self.0.read(cx);
            // The viewport canvas can change the diagram's responsive height.
            // Wait for that follow-up layout before retaining its exact extent.
            if !width_changed
                && view.viewport == layout.viewport
                && !view.fullscreen
                && view.zoom == 1.
                && view.drag.is_none()
            {
                *view.measurement.borrow_mut() = Some(Rc::new(Measurement {
                    source: view.source.clone(),
                    image: view.image.clone(),
                    ratio: view.ratio,
                    image_width: view.width,
                    viewport: view.viewport,
                    size: bounds.size,
                    rem: window.rem_size(),
                    window_width: window.viewport_size().width,
                    theme: grove_core::theme::generation(),
                    style: window.text_style(),
                }));
            }
        } else {
            // At fit zoom, Zoom out is disabled. Preserve the remaining real
            // handles in their original order, including the viewport's own
            // tab-stop policy. Focus or interaction reifies the real viewer.
            window.set_view_id(self.0.entity_id());
            let view = self.0.read(cx);
            let targets = [
                view.controls[1].clone().tab_stop(true),
                view.controls[2].clone().tab_stop(true),
                view.controls[3].clone().tab_stop(true),
                view.focus.clone(),
            ];
            for (index, focus) in targets.into_iter().enumerate() {
                let mut proxy = KeyboardProxy::new(self.0.clone(), focus, index + 1, layout.layout)
                    .into_any_element();
                proxy.request_layout(window, cx);
                proxy.prepaint(window, cx);
                layout.proxies.push(proxy);
            }
        }
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        layout: &mut Layout,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(element) = &mut layout.element {
            element.paint(window, cx);
        }
        for proxy in &mut layout.proxies {
            proxy.paint(window, cx);
        }
    }
}
struct KeyboardProxy {
    focus: FocusHandle,
    interaction: gpui::Interactivity,
    layout: LayoutId,
}
impl KeyboardProxy {
    fn new(
        owner: gpui::Entity<DiagramViewer>,
        focus: FocusHandle,
        action: usize,
        layout: LayoutId,
    ) -> Self {
        let mut element = div()
            .track_focus(&focus)
            .on_key_down(move |event, window, cx| {
                owner.update(cx, |view, cx| match action {
                    1 => view.control_key(event, view.zoom * ZOOM_STEP, window, cx),
                    2 => view.control_key(event, 1., window, cx),
                    3 if matches!(event.keystroke.key.as_str(), "enter" | "space") => {
                        view.toggle_fullscreen(window, cx);
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                    4 => view.key_down(event, window, cx),
                    _ => {}
                });
            });
        Self {
            focus,
            interaction: std::mem::take(element.interactivity()),
            layout,
        }
    }
}
impl IntoElement for KeyboardProxy {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for KeyboardProxy {
    type RequestLayoutState = ();
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
        _: &mut Window,
        _: &mut App,
    ) -> (LayoutId, ()) {
        (self.layout, ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.set_focus_handle(&self.focus, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.interaction.paint(
            None,
            None,
            Bounds::default(),
            None,
            window,
            cx,
            |_, _, _| {},
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct History {
        viewer: gpui::Entity<DiagramViewer>,
        scroll: gpui::ScrollHandle,
        before: FocusHandle,
        after: FocusHandle,
        width: Pixels,
    }
    impl Render for History {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("diagram-history")
                .w(self.width)
                .h(gpui::px(240.))
                .on_key_down(crate::views::shell::traverse_unhandled_tab)
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .child(
                    div()
                        .h(gpui::px(24.))
                        .track_focus(&self.before)
                        .child("Before"),
                )
                .child(DiagramElement(self.viewer.clone()))
                .child(div().h(gpui::px(1600.)))
                .child(
                    div()
                        .h(gpui::px(24.))
                        .track_focus(&self.after)
                        .child("After"),
                )
        }
    }
    fn draw(history: &gpui::Entity<History>, cx: &mut gpui::VisualTestContext) {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(16));
        cx.update(gpui::Window::simulate_next_frame);
        cx.update(|_, cx| history.update(cx, |_, cx| cx.notify()));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
    }
    #[gpui::test]
    fn offscreen_ready_diagram_preserves_keyboard_camera_and_responsive_geometry(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            c::sync_component_theme(cx);
            cx.set_reduce_motion(true);
        });
        let mut history = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let viewer = cx.new(|cx| DiagramViewer {
                source: "flowchart LR\nA-->B".into(),
                image: Arc::new(Image::from_bytes(ImageFormat::Png, Vec::new())),
                detail_image: None,
                ratio: 2.,
                width: 200.,
                zoom: 1.,
                pan: gpui::Point::default(),
                drag: None,
                viewport: gpui::Size::default(),
                viewport_bounds: gpui::Bounds::default(),
                focus: cx.focus_handle(),
                controls: std::array::from_fn(|_| cx.focus_handle()),
                fullscreen: false,
                return_focus: None,
                measurement: Default::default(),
            });
            let view = cx.new(|cx| History {
                viewer,
                scroll: gpui::ScrollHandle::new(),
                before: cx.focus_handle().tab_stop(true),
                after: cx.focus_handle().tab_stop(true),
                width: gpui::px(600.),
            });
            history = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        let history = history.unwrap();
        cx.simulate_resize(gpui::size(gpui::px(640.), gpui::px(300.)));
        for _ in 0..4 {
            draw(&history, cx);
        }
        let (viewer, size) = history.read_with(cx, |view, cx| {
            (
                view.viewer.clone(),
                view.viewer
                    .read(cx)
                    .measurement
                    .borrow()
                    .as_ref()
                    .expect("stable viewer geometry")
                    .size,
            )
        });
        cx.update(|_, cx| {
            history.update(cx, |view, cx| {
                view.scroll
                    .set_offset(gpui::point(gpui::px(0.), -size.height - gpui::px(100.)));
                cx.notify();
            })
        });
        draw(&history, cx);
        MATERIALIZED.with(|n| n.set(0));
        draw(&history, cx);
        assert_eq!(
            MATERIALIZED.with(|n| n.get()),
            0,
            "warm offscreen viewer must retain its measured extent"
        );
        let controls = viewer.read_with(cx, |view, _| view.controls.clone());
        cx.update(|window, cx| history.read(cx).after.clone().focus(window, cx));
        cx.simulate_keystrokes("shift-tab");
        draw(&history, cx);
        cx.update(|window, _| {
            assert!(
                controls[3].is_focused(window),
                "reverse traversal must reach Expand"
            )
        });
        cx.simulate_keystrokes("shift-tab");
        draw(&history, cx);
        cx.update(|window, _| {
            assert!(
                controls[2].is_focused(window),
                "Fit must stay in the original order"
            )
        });
        cx.simulate_keystrokes("shift-tab");
        draw(&history, cx);
        cx.update(|window, _| assert!(controls[1].is_focused(window)));
        cx.simulate_keystrokes("shift-tab");
        draw(&history, cx);
        cx.update(|window, cx| {
            assert!(
                history.read(cx).before.is_focused(window),
                "disabled Zoom out must not enter tab order"
            )
        });
        cx.simulate_keystrokes("tab");
        draw(&history, cx);
        cx.update(|window, _| assert!(controls[1].is_focused(window)));
        cx.simulate_keystrokes("enter");
        draw(&history, cx);
        assert_eq!(
            viewer.read_with(cx, |view, _| view.zoom),
            ZOOM_STEP,
            "original keyboard zoom action must execute"
        );
        cx.update(|window, cx| {
            history.read(cx).after.clone().focus(window, cx);
            viewer.update(cx, |view, cx| view.set_zoom(1., cx));
        });
        for _ in 0..3 {
            draw(&history, cx);
        }
        MATERIALIZED.with(|n| n.set(0));
        draw(&history, cx);
        assert_eq!(MATERIALIZED.with(|n| n.get()), 0);
        cx.update(|_, cx| {
            history.update(cx, |view, cx| {
                view.width = gpui::px(280.);
                cx.notify();
            })
        });
        for _ in 0..4 {
            draw(&history, cx);
        }
        let narrower = viewer.read_with(cx, |view, _| {
            view.measurement.borrow().as_ref().unwrap().size
        });
        assert!(narrower.width < size.width);
        assert!(narrower.height > gpui::px(0.));
        cx.update(|_, cx| {
            gpui_component::Theme::global_mut(cx).font_size = gpui::px(20.);
        });
        for _ in 0..4 {
            draw(&history, cx);
        }
        assert_eq!(
            viewer.read_with(cx, |view, _| view
                .measurement
                .borrow()
                .as_ref()
                .unwrap()
                .rem),
            gpui::px(20.)
        );
        MATERIALIZED.with(|n| n.set(0));
        cx.update(|_, cx| {
            viewer.update(cx, |view, cx| {
                view.source.push_str("\n%% changed");
                view.image = Arc::new(Image::from_bytes(ImageFormat::Png, Vec::new()));
                view.ratio = 3.;
                cx.notify();
            })
        });
        for _ in 0..4 {
            draw(&history, cx);
        }
        assert!(
            MATERIALIZED.with(|n| n.get()) > 0,
            "changed source/image must invalidate geometry"
        );
        cx.update(|_, cx| {
            history.update(cx, |view, cx| {
                view.scroll.set_offset(gpui::Point::default());
                cx.notify();
            })
        });
        for _ in 0..3 {
            draw(&history, cx);
        }
        let measured = viewer.read_with(cx, |view, _| {
            view.measurement.borrow().as_ref().unwrap().size
        });
        assert_eq!(
            cx.debug_bounds("transcript-mermaid-chart").unwrap().size,
            measured,
            "returning onscreen must reproduce retained geometry"
        );
    }
}
