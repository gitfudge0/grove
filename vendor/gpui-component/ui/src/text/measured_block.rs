//! Exact measured paragraph extents with lazy offscreen rendering. The outer
//! transcript retains its ordinary scroll geometry and selectable text owner.
use std::{cell::RefCell, rc::Rc, sync::Arc};

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Size, TextStyle, WeakEntity, Window, size,
};

use super::{
    TextViewState,
    document::NodeRenderOptions,
    node::{BlockNode, NodeContext},
};

#[cfg(test)]
thread_local! {
    static RENDERED_BLOCKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(super) struct MeasuredBlock {
    blocks: Arc<Vec<BlockNode>>,
    node_cx: Arc<NodeContext>,
    options: NodeRenderOptions,
    owner: WeakEntity<TextViewState>,
}

struct Measurement {
    blocks: Arc<Vec<BlockNode>>,
    index: usize,
    is_last: bool,
    text_style: TextStyle,
    paragraph_gap: gpui::Rems,
    rem_size: Pixels,
    viewport_width: Pixels,
    size: Size<Pixels>,
}

type MeasurementSlot = Rc<RefCell<Option<Measurement>>>;

pub(super) struct BlockLayout {
    measurement: MeasurementSlot,
    element: Option<AnyElement>,
    text_style: TextStyle,
    reused: bool,
    clip: Option<Bounds<Pixels>>,
}

impl MeasuredBlock {
    /// Dynamic Markdown extensions and images keep their existing eager layout.
    /// Plain prose has no external source of geometry changes between renders.
    pub(super) fn eligible(node: &BlockNode, node_cx: &NodeContext) -> bool {
        let BlockNode::Paragraph(paragraph) = node else {
            return false;
        };
        !paragraph.children.iter().any(|child| {
            child.image.is_some()
                || child.marks.iter().any(|(_, mark)| {
                    mark.link
                        .as_ref()
                        .is_some_and(|link| node_cx.markdown_extensions.may_render_link(&link.url))
                })
        })
    }

    pub(super) fn new(
        blocks: Arc<Vec<BlockNode>>,
        node_cx: Arc<NodeContext>,
        options: NodeRenderOptions,
        owner: WeakEntity<TextViewState>,
    ) -> Self {
        Self {
            blocks,
            node_cx,
            options,
            owner,
        }
    }

    fn render(&self, window: &mut Window, cx: &mut App) -> AnyElement {
        #[cfg(test)]
        RENDERED_BLOCKS.with(|count| count.set(count.get() + 1));
        self.blocks[self.options.ix].render_block(self.options, &self.node_cx, window, cx)
    }

    fn matches(&self, measured: &Measurement, window: &Window, text_style: &TextStyle) -> bool {
        measured.index == self.options.ix
            && measured.is_last == self.options.is_last
            && measured.text_style == *text_style
            && measured.paragraph_gap == self.node_cx.style.paragraph_gap
            && measured.rem_size == window.rem_size()
            && measured.viewport_width == window.viewport_size().width
            && (Arc::ptr_eq(&measured.blocks, &self.blocks)
                || measured.blocks.get(measured.index) == self.blocks.get(self.options.ix))
    }
}

impl IntoElement for MeasuredBlock {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for MeasuredBlock {
    type RequestLayoutState = BlockLayout;
    type PrepaintState = bool;

    fn id(&self) -> Option<ElementId> {
        Some(("measured-markdown-paragraph", self.options.ix).into())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, BlockLayout) {
        let measurement = window.with_element_state(
            id.expect("measured paragraph has an id"),
            |slot: Option<MeasurementSlot>, _| {
                let slot = slot.unwrap_or_default();
                (slot.clone(), slot)
            },
        );
        let text_style = window.text_style();
        let cached_size = measurement
            .borrow()
            .as_ref()
            .filter(|measured| self.matches(measured, window, &text_style))
            .map(|measured| measured.size);
        if let Some(cached_size) = cached_size {
            // Retain only the newest parsed document; unchanged paragraphs must
            // not pin a different historic AST for every streaming update.
            if let Some(measured) = measurement.borrow_mut().as_mut() {
                measured.blocks = self.blocks.clone();
            }
            let id = super::measured_body::request_extent(cached_size, window);
            (
                id,
                BlockLayout {
                    measurement,
                    element: None,
                    text_style,
                    reused: true,
                    clip: None,
                },
            )
        } else {
            let mut element = self.render(window, cx);
            let id = element.request_layout(window, cx);
            (
                id,
                BlockLayout {
                    measurement,
                    element: Some(element),
                    text_style,
                    reused: false,
                    clip: None,
                },
            )
        }
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut BlockLayout,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let width_changed = layout
            .measurement
            .borrow()
            .as_ref()
            .is_some_and(|measured| measured.size.width != bounds.size.width);
        let visible = bounds.intersects(&window.content_mask().bounds);
        let mut measured_size = bounds.size;
        if layout.reused && (visible || width_changed) {
            let mut element = self.render(window, cx);
            measured_size = element.layout_as_root(
                size(
                    AvailableSpace::Definite(bounds.size.width),
                    AvailableSpace::MaxContent,
                ),
                window,
                cx,
            );
            if (measured_size.height - bounds.size.height).abs() > gpui::px(0.25) {
                layout.clip = Some(bounds);
            }
            if visible {
                window.with_content_mask(
                    layout.clip.map(|bounds| gpui::ContentMask { bounds }),
                    |window| {
                        element.prepaint_at(bounds.origin, window, cx);
                    },
                );
                layout.element = Some(element);
            }
            // Parent-width changes can happen independently of window resizing
            // (sidebar/pane resizing). Correct the cached extent on the next
            // frame without presenting stale text wrapping in this frame.
            if (measured_size.height - bounds.size.height).abs() > gpui::px(0.25) {
                if let Some(owner) = self.owner.upgrade() {
                    super::measured_body::request_correction(&owner, window, cx);
                }
            }
        } else if visible {
            if let Some(element) = layout.element.as_mut() {
                element.prepaint(window, cx);
            }
        }
        *layout.measurement.borrow_mut() = Some(Measurement {
            blocks: self.blocks.clone(),
            index: self.options.ix,
            is_last: self.options.is_last,
            text_style: layout.text_style.clone(),
            paragraph_gap: self.node_cx.style.paragraph_gap,
            rem_size: window.rem_size(),
            viewport_width: window.viewport_size().width,
            size: measured_size,
        });
        visible
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        layout: &mut BlockLayout,
        visible: &mut bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        if *visible {
            if let Some(element) = layout.element.as_mut() {
                window.with_content_mask(
                    layout.clip.map(|bounds| gpui::ContentMask { bounds }),
                    |window| {
                        element.paint(window, cx);
                    },
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::TextView;
    use gpui::{
        AppContext as _, Context, Entity, InteractiveElement as _, ParentElement as _, Render,
        ScrollHandle, StatefulInteractiveElement as _, Styled as _, TestAppContext,
        VisualTestContext, div, px,
    };

    struct Transcript {
        owner: Entity<TextViewState>,
        scroll: ScrollHandle,
        width: Pixels,
    }

    impl Render for Transcript {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("paragraph-scroll")
                .w(self.width)
                .h(px(180.))
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .child(TextView::new(&self.owner).selectable(true))
        }
    }

    fn draw(view: &Entity<Transcript>, cx: &mut VisualTestContext) {
        cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    #[gpui::test]
    fn offscreen_paragraphs_keep_exact_extent_selection_and_resized_wrapping(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::init);
        let source = (0..160).map(|i| format!("Paragraph {i}: representative bold-free prose with several words retained for exact native selection.\n\n")).collect::<String>();
        let mut retained = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| Transcript {
                owner: cx.new(|cx| TextViewState::markdown(&source, cx)),
                scroll: ScrollHandle::new(),
                width: px(360.),
            });
            retained = Some(view.clone());
            crate::Root::new(view, window, cx).bordered(false)
        });
        let view = retained.unwrap();
        draw(&view, cx);
        let original_extent = view.read_with(cx, |view, _| view.scroll.max_offset().y);
        RENDERED_BLOCKS.with(|count| count.set(0));
        draw(&view, cx);
        let warm_rendered = RENDERED_BLOCKS.with(|count| count.get());
        assert!(
            warm_rendered < 40,
            "offscreen paragraphs must not render: {warm_rendered}"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.scroll.max_offset().y),
            original_extent
        );
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.scroll.scroll_to_bottom();
                cx.notify();
            })
        });
        draw(&view, cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.width = px(170.);
                cx.notify();
            })
        });
        draw(&view, cx);
        // Parent-only width changes get one correction frame; they do not
        // depend on resizing the platform window or changing source bytes.
        cx.update(Window::simulate_next_frame);
        draw(&view, cx);
        let narrow_extent = view.read_with(cx, |view, _| view.scroll.max_offset().y);
        assert!(narrow_extent > original_extent);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.owner.update(cx, |owner, cx| owner.select_all(cx));
                cx.notify();
            })
        });
        draw(&view, cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.scroll.max_offset().y,
                narrow_extent,
                "cached and fully eager native layout must agree"
            );
            let copied = view.owner.read(cx).selected_text();
            for i in 0..160 {
                assert_eq!(copied.matches(&format!("Paragraph {i}:")).count(), 1);
            }
        });
    }
}
