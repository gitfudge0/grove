//! Decorative motion invalidates a leaf entity rather than its owning screen.
use gpui::{
    prelude::*, AnyElement, App, Bounds, Context, Element, ElementId, Entity, GlobalElementId,
    InspectorElementId, LayoutId, Pixels, Window,
};
use std::rc::Rc;

type Draw = Rc<dyn Fn(&mut Window, &mut Context<AnimationLeaf>) -> AnyElement>;

pub(crate) struct AnimationLeaf {
    draw: Draw,
}
impl Render for AnimationLeaf {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        (self.draw.clone())(window, cx)
    }
}

/// The keyed holder is never notified. In particular, it must not be the leaf
/// itself: use_keyed_state observes its entity and forwards notifications to the
/// original screen, which would undo the invalidation boundary.
struct Holder(Entity<AnimationLeaf>);

pub(crate) fn isolated_animation(
    id: ElementId,
    draw: impl Fn(&mut Window, &mut Context<AnimationLeaf>) -> AnyElement + 'static,
) -> AnyElement {
    IsolatedAnimation {
        id,
        draw: Rc::new(draw),
    }
    .into_any_element()
}

struct IsolatedAnimation {
    id: ElementId,
    draw: Draw,
}
impl IntoElement for IsolatedAnimation {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for IsolatedAnimation {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
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
    ) -> (LayoutId, AnyElement) {
        let holder = window.use_keyed_state("animation-holder", cx, |_, cx| {
            Holder(cx.new(|_| AnimationLeaf {
                draw: self.draw.clone(),
            }))
        });
        let leaf = holder.read(cx).0.clone();
        leaf.update(cx, |leaf, _| leaf.draw = self.draw.clone());
        let mut element = leaf.into_any_element();
        let layout = element.request_layout(window, cx);
        (layout, element)
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        element: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.prepaint(window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        element: &mut AnyElement,
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        element.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{div, Animation, AnimationExt as _, StyleRefinement, TestAppContext};
    use std::cell::Cell;

    struct Screen(Rc<Cell<usize>>, Rc<Cell<usize>>);
    impl Render for Screen {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.0.set(self.0.get() + 1);
            let leaf = self.1.clone();
            div()
                .size_full()
                .child(isolated_animation("leaf".into(), move |_, _| {
                    leaf.set(leaf.get() + 1);
                    div()
                        .size_full()
                        .with_animation(
                            "pulse",
                            Animation::new(std::time::Duration::from_secs(10)).repeat(),
                            gpui::Styled::opacity,
                        )
                        .into_any_element()
                }))
        }
    }
    struct Static(Rc<Cell<usize>>);
    impl Render for Static {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.0.set(self.0.get() + 1);
            div().size_full()
        }
    }
    struct Root(Entity<Screen>, Entity<Static>);
    impl Render for Root {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .child(
                    self.0.clone().cached(
                        StyleRefinement::default()
                            .w(gpui::px(50.))
                            .h(gpui::px(100.)),
                    ),
                )
                .child(
                    self.1.clone().cached(
                        StyleRefinement::default()
                            .w(gpui::px(50.))
                            .h(gpui::px(100.)),
                    ),
                )
        }
    }
    /// The pinned engine dirties ancestors of animated views. This intentionally
    /// records that limitation while proving unrelated cached screens are reused.
    #[gpui::test]
    fn isolated_animation_reuses_siblings_but_pinned_engine_renders_ancestors(
        cx: &mut TestAppContext,
    ) {
        let screen = Rc::new(Cell::new(0));
        let leaf = Rc::new(Cell::new(0));
        let sibling = Rc::new(Cell::new(0));
        let window = cx.open_window(gpui::size(gpui::px(100.), gpui::px(100.)), |_, cx| {
            Root(
                cx.new(|_| Screen(screen.clone(), leaf.clone())),
                cx.new(|_| Static(sibling.clone())),
            )
        });
        cx.run_until_parked();
        let first = (screen.get(), leaf.get(), sibling.get());
        assert!(first.0 > 0 && first.1 > 0 && first.2 > 0);
        for _ in 0..3 {
            assert!(
                window
                    .update(cx, |_, window, cx| window.simulate_next_frame(cx))
                    .unwrap()
                    > 0
            );
            cx.run_until_parked();
        }
        assert!(
            screen.get() > first.0,
            "GPUI still propagates dirty state to ancestors"
        );
        assert!(leaf.get() > first.1, "required pulse keeps animating");
        assert_eq!(
            sibling.get(),
            first.2,
            "cached unrelated screen stays unchanged"
        );
    }
    #[gpui::test]
    fn isolated_animation_reduced_motion_schedules_no_frames(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let window = cx.open_window(gpui::size(gpui::px(100.), gpui::px(100.)), |_, _cx| {
            Screen(Rc::default(), Rc::default())
        });
        cx.run_until_parked();
        assert_eq!(
            window
                .update(cx, |_, window, cx| window.simulate_next_frame(cx))
                .unwrap(),
            0
        );
    }
}
