//! Reuse the extent of static Markdown bodies while keeping the outer TextView
//! alive for selection ownership, focus routing and current scroll geometry.
use std::sync::Arc;

use gpui::{
    AnyElement, App, Bounds, ContentMask, Element, ElementId, Entity, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Size, TextStyle, Window,
};

use super::{MarkdownExtensions, TextViewState, TextViewStyle, node::BlockNode};

/// Features of the accepted immutable document, independent of its layout.
/// Repeated frame/projection queries never walk the Markdown tree.
#[derive(Default)]
pub(super) struct DocumentCodeBlocks {
    blocks: Option<Arc<Vec<BlockNode>>>,
    contains_code: bool,
    #[cfg(test)]
    scans: usize,
}

impl DocumentCodeBlocks {
    pub(super) fn accept(&mut self, blocks: &Arc<Vec<BlockNode>>) {
        if self
            .blocks
            .as_ref()
            .is_some_and(|old| Arc::ptr_eq(old, blocks))
        {
            return;
        }
        self.contains_code = blocks.iter().any(contains_code_block);
        self.blocks = Some(blocks.clone());
        #[cfg(test)]
        {
            self.scans += 1;
        }
    }
    pub(super) fn contains_code(&self) -> bool {
        self.contains_code
    }
}

fn contains_code_block(block: &BlockNode) -> bool {
    match block {
        BlockNode::CodeBlock(_) => true,
        BlockNode::Root { children, .. }
        | BlockNode::Blockquote { children, .. }
        | BlockNode::List { children, .. }
        | BlockNode::ListItem { children, .. } => children.iter().any(contains_code_block),
        _ => false,
    }
}

#[derive(Clone)]
pub(super) struct BodyMeasurement {
    blocks: Arc<Vec<BlockNode>>,
    extensions: u64,
    style: TextViewStyle,
    text_style: TextStyle,
    rem_size: Pixels,
    viewport_width: Pixels,
    size: Size<Pixels>,
    stable: bool,
    supported: bool,
}

impl BodyMeasurement {
    pub(super) fn projectable_for(&self, blocks: &Arc<Vec<BlockNode>>, extensions: u64) -> bool {
        self.stable
            && self.supported
            && Arc::ptr_eq(&self.blocks, blocks)
            && self.extensions == extensions
    }
}

pub(super) fn window_selection_active(window: &Window, cx: &App) -> bool {
    window.root::<crate::Root>().flatten().is_some_and(|root| {
        let selection = &root.read(cx).text_selection;
        selection.is_selecting
            || (selection.did_hit_text
                && match (&selection.anchor, &selection.cursor) {
                    (Some(anchor), Some(cursor)) => {
                        let alive = |endpoint: &super::window_selection::SelectionEndpoint| {
                            endpoint
                                .view
                                .as_ref()
                                .is_none_or(|view| view.upgrade().is_some())
                        };
                        alive(anchor)
                            && alive(cursor)
                            && (anchor.point != cursor.point
                                || anchor.view.as_ref().map(|view| view.entity_id())
                                    != cursor.view.as_ref().map(|view| view.entity_id()))
                    }
                    _ => false,
                })
    })
}

fn stable_block(block: &BlockNode, extensions: &MarkdownExtensions) -> bool {
    let paragraph = match block {
        BlockNode::Paragraph(paragraph)
        | BlockNode::Heading {
            children: paragraph,
            ..
        } => paragraph,
        BlockNode::Root { children, .. }
        | BlockNode::Blockquote { children, .. }
        | BlockNode::List { children, .. }
        | BlockNode::ListItem { children, .. } => {
            return children.iter().all(|block| stable_block(block, extensions));
        }
        BlockNode::CodeBlock(_)
        | BlockNode::Break { .. }
        | BlockNode::HorizontalRule { .. }
        | BlockNode::Definition { .. }
        | BlockNode::Unknown => return true,
        // Images, custom renderers and tables retain their dynamic native layout.
        _ => return false,
    };
    !paragraph.children.iter().any(|child| {
        child.image.is_some()
            || child.marks.iter().any(|(_, mark)| {
                mark.link
                    .as_ref()
                    .is_some_and(|link| extensions.may_render_link(&link.url))
            })
    })
}

pub(super) fn record_layout(
    state: &mut TextViewState,
    bounds: Bounds<Pixels>,
    text_style: TextStyle,
    window: &Window,
) {
    // A nested paragraph may discover a new height during prepaint, after
    // its parent's layout was computed. Do not cache that superseded extent.
    if state.layout_correction_pending {
        state.measured_layout = None;
        return;
    }
    let blocks = state.parsed_content.document.blocks.clone();
    let extensions = state.markdown_extensions.revision();
    let stable = state
        .measured_layout
        .as_ref()
        .filter(|old| Arc::ptr_eq(&old.blocks, &blocks) && old.extensions == extensions)
        .map_or_else(
            || {
                blocks
                    .iter()
                    .all(|block| stable_block(block, &state.markdown_extensions))
            },
            |old| old.stable,
        );
    state.measured_layout = Some(BodyMeasurement {
        blocks,
        extensions,
        style: state.text_view_style.clone(),
        text_style,
        rem_size: window.rem_size(),
        viewport_width: window.viewport_size().width,
        size: bounds.size,
        stable: stable && state.text_view_style.heading_font_size.is_none(),
        supported: state.body_measurement_supported(),
    });
}

pub(super) fn request_correction(owner: &Entity<TextViewState>, window: &Window, cx: &mut App) {
    let schedule = owner.update(cx, |state, _| {
        state.measured_layout = None;
        !std::mem::replace(&mut state.layout_correction_pending, true)
    });
    if schedule {
        let owner = owner.downgrade();
        window.on_next_frame(move |_, cx| {
            let _ = owner.update(cx, |state, cx| {
                state.layout_correction_pending = false;
                cx.notify();
            });
        });
    }
}

#[cfg(test)]
thread_local! { static CULLED_BODIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

/// Preserve intrinsic width during min/max-content probes. A percentage-only
/// spacer measures as zero in shrink-wrapped parents such as absolute overlays.
pub(super) fn request_extent(cached: Size<Pixels>, window: &mut Window) -> LayoutId {
    let mut style = gpui::Style::default();
    style.size.width = gpui::relative(1.).into();
    window.request_measured_layout(style, move |known, available, _, _| {
        let width = known.width.unwrap_or_else(|| match available.width {
            gpui::AvailableSpace::Definite(width) => cached.width.min(width),
            gpui::AvailableSpace::MinContent | gpui::AvailableSpace::MaxContent => cached.width,
        });
        gpui::size(width, known.height.unwrap_or(cached.height))
    })
}

pub(super) struct MeasuredBody {
    state: Entity<TextViewState>,
}

pub(super) struct BodyLayout {
    element: Option<AnyElement>,
    cached_size: Option<Size<Pixels>>,
    force_eager: bool,
    clip: Option<Bounds<Pixels>>,
}

impl MeasuredBody {
    pub(super) fn new(state: Entity<TextViewState>) -> Self {
        Self { state }
    }
}

impl IntoElement for MeasuredBody {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for MeasuredBody {
    type RequestLayoutState = BodyLayout;
    type PrepaintState = bool;
    fn id(&self) -> Option<ElementId> {
        Some("measured-markdown-body".into())
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
    ) -> (LayoutId, BodyLayout) {
        let window_selection = window_selection_active(window, cx);
        let state = self.state.read(cx);
        let current_extent = state
            .body_measurement_supported()
            .then(|| state.measured_layout.as_ref())
            .flatten()
            .filter(|old| {
                old.projectable_for(
                    &state.parsed_content.document.blocks,
                    state.markdown_extensions.revision(),
                ) && old.style == state.text_view_style
                    && old.style.code_block == state.text_view_style.code_block
                    && old.style.table == state.text_view_style.table
                    && old.style.table_cell == state.text_view_style.table_cell
                    && old.text_style == window.text_style()
                    && old.rem_size == window.rem_size()
                    && old.viewport_width == window.viewport_size().width
            })
            .map(|old| old.size);
        // Tab stops are registered during paint. Unknown/currently dynamic
        // bodies keep their full tree, including custom and code-block actions.
        let force_eager = current_extent.is_none()
            || window.is_a11y_active()
            || state.has_view_selection()
            || state.is_selecting
            || window_selection
            || state.focus_handle.contains_focused(window, cx)
            || window.is_inspector_picking(cx);
        let cached_size = (!force_eager).then_some(current_extent).flatten();
        if let Some(size) = cached_size {
            let id = request_extent(size, window);
            (
                id,
                BodyLayout {
                    element: None,
                    cached_size,
                    force_eager,
                    clip: None,
                },
            )
        } else {
            let mut element = self.state.clone().into_any_element();
            let id = element.request_layout(window, cx);
            (
                id,
                BodyLayout {
                    element: Some(element),
                    cached_size: None,
                    force_eager,
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
        layout: &mut BodyLayout,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let visible = bounds.intersects(&window.content_mask().bounds);
        if let Some(cached_size) = layout.cached_size {
            let width_changed = cached_size.width != bounds.size.width;
            if visible || width_changed {
                let mut element = self.state.clone().into_any_element();
                // Preserve the parent's exact allocated height (including a
                // fixed-height text region). Width changes invalidate the next
                // layout pass, which can then resolve the original constraints.
                element.layout_as_root(bounds.size.into(), window, cx);
                if width_changed {
                    layout.clip = Some(bounds);
                    request_correction(&self.state, window, cx);
                }
                window.with_content_mask(
                    layout.clip.map(|bounds| ContentMask { bounds }),
                    |window| {
                        element.prepaint_at(bounds.origin, window, cx);
                    },
                );
                layout.element = Some(element);
            } else {
                // TextView's outer paint still registers this owner/hitbox and
                // clears its old inline hit bounds. Keep content anchors current.
                #[cfg(test)]
                CULLED_BODIES.with(|count| count.set(count.get() + 1));
                self.state
                    .update(cx, |state, _| state.update_bounds(bounds));
            }
        } else if let Some(element) = layout.element.as_mut() {
            element.prepaint(window, cx);
        }
        visible || layout.force_eager
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        layout: &mut BodyLayout,
        paint: &mut bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        if *paint {
            if let Some(element) = layout.element.as_mut() {
                window.with_content_mask(
                    layout.clip.map(|bounds| ContentMask { bounds }),
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
    use crate::{WindowExt as _, text::TextView};
    use gpui::{
        AppContext as _, Context, FocusHandle, InteractiveElement as _, ParentElement as _, Render,
        ScrollHandle, StatefulInteractiveElement as _, Styled as _, TestAppContext,
        VisualTestContext, div, px,
    };

    struct History {
        owners: Vec<Entity<TextViewState>>,
        scroll: ScrollHandle,
        width: Pixels,
    }
    impl Render for History {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("body-history")
                .w(self.width)
                .h(px(180.))
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .children(self.owners.iter().map(|owner| {
                            div()
                                .w_full()
                                .flex_shrink_0()
                                .child(TextView::new(owner).selectable(true))
                        })),
                )
        }
    }
    fn draw<T: Render>(view: &Entity<T>, cx: &mut VisualTestContext) {
        cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    #[test]
    fn accepted_document_code_detection_is_cached_by_arc_identity() {
        let code = Arc::new(vec![BlockNode::Root {
            children: vec![BlockNode::ListItem {
                children: vec![BlockNode::Blockquote {
                    children: vec![BlockNode::CodeBlock(crate::text::node::CodeBlock::new(
                        "printf guarded".into(),
                        Some("sh".into()),
                        None::<crate::text::node::Span>,
                    ))],
                    span: None,
                }],
                spread: false,
                checked: None,
                span: None,
            }],
            span: None,
        }]);
        let mut features = DocumentCodeBlocks::default();
        features.accept(&code);
        for _ in 0..100 {
            assert!(features.contains_code());
            features.accept(&code);
        }
        assert_eq!(features.scans, 1, "one scan for the accepted document Arc");
        let new_document = Arc::new(code.as_ref().clone());
        features.accept(&new_document);
        assert!(features.contains_code());
        assert_eq!(features.scans, 2);
        features.accept(&Arc::new(Vec::new()));
        assert!(!features.contains_code());
        assert_eq!(features.scans, 3);
    }

    struct ActionHistory {
        owner: Entity<TextViewState>,
        plain: Entity<TextViewState>,
        action: FocusHandle,
        after: FocusHandle,
        scroll: ScrollHandle,
        actions_enabled: bool,
        custom: Option<MarkdownExtensions>,
    }
    impl Render for ActionHistory {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let mut text = TextView::new(&self.owner).selectable(true);
            if let Some(custom) = &self.custom {
                text = text.markdown_extensions(custom.clone());
            } else if self.actions_enabled {
                let focus = self.action.clone();
                text = text.code_block_actions(move |_, _, _| {
                    div()
                        .id("guard-code-action")
                        .debug_selector(|| "guard-code-action".into())
                        .track_focus(&focus)
                        .tab_index(0)
                        .child("Copy code")
                });
            }
            div()
                .id("action-history")
                .w(px(360.))
                .h(px(180.))
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(div().w_full().flex_shrink_0().child(text))
                        .child(div().h(px(1000.)).flex_shrink_0())
                        .child(
                            div().w_full().flex_shrink_0().child(
                                TextView::new(&self.plain).code_block_actions(|_, _, _| div()),
                            ),
                        )
                        .child(
                            div()
                                .id("after-action-history")
                                .track_focus(&self.after)
                                .tab_index(0)
                                .flex_shrink_0()
                                .child("After history"),
                        ),
                )
        }
    }
    fn action_history_fixture(
        cx: &mut TestAppContext,
        source: String,
    ) -> (Entity<ActionHistory>, &mut VisualTestContext) {
        cx.update(crate::init);
        let mut retained = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| ActionHistory {
                owner: cx.new(|cx| TextViewState::markdown(&source, cx)),
                plain: cx.new(|cx| TextViewState::markdown("Plain prose stays cacheable.", cx)),
                // Explicit tracked handles keep their own tab-stop flags;
                // Div::tab_index only configures handles created by the Div.
                action: cx.focus_handle().tab_stop(true).tab_index(0),
                after: cx.focus_handle().tab_stop(true).tab_index(0),
                scroll: ScrollHandle::new(),
                actions_enabled: false,
                custom: None,
            });
            retained = Some(view.clone());
            crate::Root::new(view, window, cx).bordered(false)
        });
        let view = retained.unwrap();
        draw(&view, cx);
        draw(&view, cx);
        (view, cx)
    }
    fn assert_offscreen_action_tab_stop(view: &Entity<ActionHistory>, cx: &mut VisualTestContext) {
        let bounds = cx
            .debug_bounds("guard-code-action")
            .expect("offscreen action still painted");
        view.read_with(cx, |view, cx| {
            assert!(
                bounds.bottom() < view.scroll.bounds().top(),
                "action must be offscreen"
            );
            assert!(!view.owner.read(cx).supports_layout_projection());
            assert!(
                view.plain.read(cx).supports_layout_projection(),
                "plain prose with callback retains caching"
            );
        });
        let (action, after) =
            view.read_with(cx, |view, _| (view.action.clone(), view.after.clone()));
        cx.update(|window, cx| {
            window.focus(&after, cx);
            window.focus_prev(cx);
            assert!(
                action.is_focused(window),
                "Tab traversal must reach actual offscreen action"
            );
        });
    }

    #[gpui::test]
    fn offscreen_code_actions_keep_tab_controls_and_disable_body_projection(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = action_history_fixture(cx, "```sh\nprintf guarded\n```".into());
        view.read_with(cx, |view, cx| {
            assert!(
                view.owner.read(cx).supports_layout_projection(),
                "code without actions remains cacheable"
            );
        });
        let owner = view.read_with(cx, |view, _| view.owner.clone());
        owner.update(cx, |owner, _| {
            let blocks = owner.parsed_content.document.blocks.clone();
            let extensions = owner.markdown_extensions.clone();
            owner.markdown_extensions = Arc::new(
                MarkdownExtensions::default().block_renderer("unused-action", |_, _, _| div()),
            );
            assert!(Arc::ptr_eq(&blocks, &owner.parsed_content.document.blocks));
            assert!(
                !owner.supports_layout_projection(),
                "extension-only changes must invalidate the public projection predicate",
            );
            owner.markdown_extensions = extensions;
            assert!(owner.supports_layout_projection());
        });
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.actions_enabled = true;
                cx.notify();
            })
        });
        draw(&view, cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.scroll.scroll_to_bottom();
                cx.notify();
            })
        });
        draw(&view, cx);
        draw(&view, cx);
        assert_offscreen_action_tab_stop(&view, cx);
    }

    #[gpui::test]
    fn offscreen_custom_actions_keep_tab_controls_after_plain_document(cx: &mut TestAppContext) {
        let (view, cx) = action_history_fixture(cx, "Dynamic action".into());
        view.read_with(cx, |view, cx| {
            assert!(view.owner.read(cx).supports_layout_projection())
        });
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.scroll.scroll_to_bottom();
                cx.notify();
            })
        });
        draw(&view, cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                let focus = view.action.clone();
                view.custom = Some(
                    MarkdownExtensions::default()
                        .block_parser(|node, context| {
                            matches!(node, markdown::mdast::Node::Paragraph(_)).then(|| {
                                crate::text::MarkdownNode::new("guard-action", ())
                                    .text("Dynamic action")
                                    .markdown(context.node_source(node).unwrap_or_default())
                            })
                        })
                        .block_renderer("guard-action", move |_, _, _| {
                            div()
                                .id("guard-code-action")
                                .debug_selector(|| "guard-code-action".into())
                                .track_focus(&focus)
                                .tab_index(0)
                                .child("Custom action")
                        }),
                );
                cx.notify();
            })
        });
        draw(&view, cx);
        assert_offscreen_action_tab_stop(&view, cx);
    }

    #[gpui::test]
    fn cached_bodies_keep_current_hitboxes_selection_and_parent_resize(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let mut retained = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| History {
                owners: (0..100).map(|i| cx.new(|cx| TextViewState::markdown(
                    &format!("## Message {i}\n\nBody {i}: representative prose retained for native selection."), cx))).collect(),
                scroll: ScrollHandle::new(), width: px(360.),
            });
            retained = Some(view.clone());
            crate::Root::new(view, window, cx).bordered(false)
        });
        let view = retained.unwrap();
        draw(&view, cx);
        CULLED_BODIES.with(|count| count.set(0));
        draw(&view, cx);
        assert!(
            CULLED_BODIES.with(|count| count.get()) > 50,
            "most offscreen bodies must reuse their extent"
        );
        let original_extent = view.read_with(cx, |view, _| view.scroll.max_offset().y);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.scroll.scroll_to_bottom();
                cx.notify();
            })
        });
        draw(&view, cx);
        cx.update(|window, cx| {
            view.read_with(cx, |view, cx| {
                let root = window.root::<crate::Root>().flatten().unwrap();
                let root = root.read(cx);
                let first = view.owners[0].read(cx).bounds();
                assert!(first.bottom() < px(0.));
                assert_eq!(
                    root.selectable_text_views[&view.owners[0].entity_id()]
                        .1
                        .bounds,
                    first
                );
                assert!(
                    !root
                        .selectable_text_inlines
                        .contains_key(&view.owners[0].entity_id())
                );
                assert!(!root.selectable_text_inlines[&view.owners[99].entity_id()].is_empty());
            });
        });
        // A zero-length click must not disable all history caches, and checking
        // its endpoints must be safe while a contributing TextView is leased.
        cx.update(|window, cx| {
            let root = window.root::<crate::Root>().flatten().unwrap();
            let last = view.read(cx).owners[99].clone();
            let hit = root.read(cx).selectable_text_inlines[&last.entity_id()][0].center();
            root.update(cx, |root, cx| {
                root.start_text_selection(hit, window, cx);
                root.end_text_selection(cx);
            });
            last.update(cx, |_, cx| assert!(!window_selection_active(window, cx)));
            window.clear_text_selection(cx);
        });
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.width = px(170.);
                cx.notify();
            })
        });
        draw(&view, cx);
        cx.update(Window::simulate_next_frame);
        draw(&view, cx);
        let cached_extent = view.read_with(cx, |view, _| view.scroll.max_offset().y);
        assert!(cached_extent > original_extent);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                for owner in &view.owners {
                    owner.update(cx, |owner, cx| owner.select_all(cx));
                }
                cx.notify();
            })
        });
        draw(&view, cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.scroll.max_offset().y,
                cached_extent,
                "cached height must equal fully eager native layout"
            );
            for (i, owner) in view.owners.iter().enumerate() {
                assert!(
                    owner
                        .read(cx)
                        .selected_text()
                        .contains(&format!("Body {i}:"))
                );
            }
        });
    }
}
