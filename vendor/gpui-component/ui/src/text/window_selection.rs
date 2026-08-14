use gpui::{
    App, Bounds, Context, Element, ElementId, Entity, EntityId, GlobalElementId, Hitbox,
    InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, ScrollWheelEvent, Style, WeakEntity, Window,
};

use crate::{Root, global_state::GlobalState, scroll::AutoScroll, text::TextViewState};

/// The modal layer a selectable [`TextView`](crate::text::TextView) belongs to.
/// Confines window selection to the active modal (Dialog/Sheet) so a drag leaving it cannot select TextViews behind it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SelectionScope {
    /// The base window content, outside any Dialog/Sheet.
    Base,
    /// A Dialog at the given layer index (matches `Dialog::layer_ix`, i.e. the
    /// position in `Root::active_dialogs`).
    Dialog(usize),
    /// The active Sheet.
    Sheet,
}

/// Extension trait that confines window text selection started inside an
/// element's subtree to a modal [`SelectionScope`]:
///
/// ```ignore
/// v_flex().child(content).selection_scope(SelectionScope::Dialog(layer_ix))
/// ```
pub(crate) trait SelectionScopeElement: IntoElement + Sized {
    fn selection_scope(self, scope: SelectionScope) -> SelectionScopeMarker<Self::Element> {
        SelectionScopeMarker {
            scope,
            element: self.into_element(),
        }
    }
}

impl<E: IntoElement> SelectionScopeElement for E {}

/// A layout-transparent wrapper element that marks its subtree with a
/// [`SelectionScope`] during paint, so selectable [`TextView`](crate::text::TextView)s painted inside it register under that scope.
pub(crate) struct SelectionScopeMarker<E> {
    scope: SelectionScope,
    element: E,
}

impl<E: Element> IntoElement for SelectionScopeMarker<E> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<E: Element> Element for SelectionScopeMarker<E> {
    type RequestLayoutState = E::RequestLayoutState;
    type PrepaintState = E::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        self.element.id()
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        self.element.source_location()
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.element.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.element
            .prepaint(id, inspector_id, bounds, request_layout, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Mark the subtree so selectable TextViews register under this scope
        // during their paint; paint is depth-first, so the bracket is exact.
        GlobalState::global_mut(cx).push_selection_scope(self.scope);
        self.element.paint(
            id,
            inspector_id,
            bounds,
            request_layout,
            prepaint,
            window,
            cx,
        );
        GlobalState::global_mut(cx).pop_selection_scope();
    }
}

/// Window-level text selection state, owned by [`Root`].
/// Selection endpoints are content-anchored, so the selection follows the content when it scrolls.
#[derive(Default)]
pub struct WindowTextSelection {
    pub(crate) anchor: Option<SelectionEndpoint>,
    pub(crate) cursor: Option<SelectionEndpoint>,
    pub(crate) is_selecting: bool,
    pub(crate) did_hit_text: bool,
}

/// A selection endpoint, content-anchored to a TextView.
/// `point` is in content coordinates so selection follows content when scrolled; blank-space presses proxy-anchor to the nearest view (`inside` false).
#[derive(Clone)]
pub(crate) struct SelectionEndpoint {
    /// Some: anchored to this TextView, `point` in its content coordinates.
    /// None: no view registered; `point` is window coordinates.
    pub(crate) view: Option<WeakEntity<TextViewState>>,
    pub(crate) point: Point<Pixels>,
    /// True when the press hit the view's hitbox; false when proxy-anchored
    /// to the nearest view from blank space.
    pub(crate) inside: bool,
    /// True when the endpoint hit an Inline text run, not just blank space in
    /// the parent TextView bounds.
    pub(crate) inside_text: bool,
}

impl SelectionEndpoint {
    /// Resolve this endpoint to window coordinates, using the view's current
    /// `bounds().origin + scroll_offset()` so it follows the content as it moves.
    fn resolve(&self, cx: &App) -> Option<Point<Pixels>> {
        match &self.view {
            Some(view) => {
                let state = view.upgrade()?;
                let state = state.read(cx);
                Some(self.point + state.scroll_offset() + state.bounds().origin)
            }
            None => Some(self.point),
        }
    }

    fn view_id(&self) -> Option<EntityId> {
        self.view.as_ref().map(|view| view.entity_id())
    }
}

impl WindowTextSelection {
    /// The (anchor, cursor) points in window coordinates, `None` if the
    /// selection is empty.
    pub(crate) fn resolved_points(&self, cx: &App) -> Option<(Point<Pixels>, Point<Pixels>)> {
        if !self.did_hit_text {
            return None;
        }
        let start = self.anchor.as_ref()?.resolve(cx)?;
        let end = self.cursor.as_ref()?.resolve(cx)?;
        if start == end {
            return None;
        }
        Some((start, end))
    }

    /// If both endpoints are anchored to the same TextView, return its id.
    /// Single-view fast path: only that view participates; otherwise all views do.
    pub(crate) fn single_view(&self) -> Option<EntityId> {
        let anchor = self.anchor.as_ref()?.view_id()?;
        let cursor = self.cursor.as_ref()?.view_id()?;
        (anchor == cursor).then_some(anchor)
    }

    fn involves(&self, view_id: EntityId) -> bool {
        self.anchor.as_ref().and_then(|e| e.view_id()) == Some(view_id)
            || self.cursor.as_ref().and_then(|e| e.view_id()) == Some(view_id)
    }
}

impl Root {
    /// Register a selectable TextView for window-level selection.
    /// Called from TextView's paint on every frame.
    pub(crate) fn register_selectable_text_view(
        state: &Entity<TextViewState>,
        hitbox: &Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(root) = window.root::<Root>().flatten() else {
            return;
        };
        let id = state.entity_id();
        let weak = state.downgrade();
        let hitbox = hitbox.clone();
        // Capture the modal scope this view is painting under (set by the
        // `SelectionScopeMarker` wrapping a Dialog/Sheet content subtree).
        let scope = GlobalState::global(cx).current_selection_scope();
        root.update(cx, |root, _| {
            // Prune dead views on each registration; O(N²) per frame, fine for
            // typical view counts — revisit for windows with hundreds of them.
            root.selectable_text_views
                .retain(|_, (view, _, _)| view.upgrade().is_some());
            root.selectable_text_views.insert(id, (weak, hitbox, scope));
            root.selectable_text_inlines.remove(&id);
        });
    }

    /// Register Inline text bounds for a selectable TextView.
    /// Called from Inline's paint on every frame.
    pub(crate) fn register_selectable_text_inline(
        state: &Entity<TextViewState>,
        text_bounds: Vec<Bounds<Pixels>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if text_bounds.is_empty() {
            return;
        }
        let Some(root) = window.root::<Root>().flatten() else {
            return;
        };
        let id = state.entity_id();
        root.update(cx, |root, _| {
            root.selectable_text_inlines
                .entry(id)
                .or_default()
                .extend(text_bounds);
        });
    }

    /// Whether there is an active text selection (window-level or view-local).
    pub(crate) fn has_text_selection(&self, cx: &App) -> bool {
        if self.text_selection.resolved_points(cx).is_some() {
            return true;
        }
        self.selectable_text_views.values().any(|(view, _, _)| {
            view.upgrade()
                .is_some_and(|view| view.read(cx).has_view_selection())
        })
    }

    /// Internal: uses `&self` so it is safe to call while Root is leased.
    /// Reflects the last painted frame; a racing copy may see stale state.
    pub(crate) fn window_selected_text(&self, cx: &App) -> String {
        let resolved = self.text_selection.resolved_points(cx);
        let single_view = self.text_selection.single_view();
        // A window selection lives in exactly one scope; only views in that
        // scope contribute, so copying never mixes text across layers.
        let anchor_scope = self.active_selection_scope();

        let mut items: Vec<(Point<Pixels>, String)> = Vec::new();
        for (id, (view, _, scope)) in self.selectable_text_views.iter() {
            let Some(view) = view.upgrade() else { continue };
            let state = view.read(cx);
            let in_window_selection = resolved.is_some()
                && state.is_selectable()
                && *scope == anchor_scope
                && single_view.map_or(true, |v| v == *id);
            if !state.has_view_selection() && !in_window_selection {
                continue;
            }
            let text = state.selected_text();
            if text.trim().is_empty() {
                continue;
            }
            items.push((state.bounds().origin, text));
        }

        items.sort_by(|a, b| {
            a.0.y
                .partial_cmp(&b.0.y)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(
                    a.0.x
                        .partial_cmp(&b.0.x)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
        });

        items
            .into_iter()
            .map(|(_, text)| text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Clear the window selection and all view-local selections.
    pub fn clear_text_selection(&mut self, cx: &mut Context<Self>) {
        let had_window_selection = self.text_selection.anchor.is_some();
        self.text_selection.anchor = None;
        self.text_selection.cursor = None;
        self.text_selection.is_selecting = false;
        self.text_selection.did_hit_text = false;
        self.selectable_text_views.retain(|_, (view, _, _)| {
            let Some(view) = view.upgrade() else {
                return false;
            };
            // Skip views with nothing to clear, to avoid re-rendering every
            // selectable view on every click.
            if had_window_selection || view.read(cx).has_view_selection() {
                view.update(cx, |state, cx| {
                    state.is_selecting = false;
                    state.clear_selection(cx);
                });
            }
            true
        });
        self.selectable_text_inlines
            .retain(|id, _| self.selectable_text_views.contains_key(id));
    }

    /// Clear the window selection when an anchored view resizes (content
    /// coordinates are stale); an active drag is not interrupted.
    pub(crate) fn clear_text_selection_for_resized_view(
        &mut self,
        view_id: EntityId,
        cx: &mut Context<Self>,
    ) {
        if self.text_selection.is_selecting {
            return;
        }
        if self.text_selection.involves(view_id) {
            self.clear_text_selection(cx);
        }
    }

    pub(crate) fn start_text_selection(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let endpoint = self.text_selection_endpoint(position, window, cx);
        // The caller already checks `suppress_text_selection` before calling this.
        // Only focus the view when the press actually hit it; a proxy-anchored endpoint must not steal focus.
        if endpoint.inside {
            if let Some(view) = endpoint.view.as_ref().and_then(|v| v.upgrade()) {
                view.update(cx, |state, cx| {
                    state.is_selecting = true;
                    state.focus_handle.focus(window, cx);
                });
            }
        }
        self.text_selection.anchor = Some(endpoint.clone());
        self.text_selection.cursor = Some(endpoint);
        self.text_selection.did_hit_text = self
            .text_selection
            .anchor
            .as_ref()
            .is_some_and(|endpoint| endpoint.inside_text);
        self.text_selection.is_selecting = true;
    }

    pub(crate) fn update_text_selection(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.text_selection.is_selecting {
            return;
        }
        // Do not update the selection while a GPUI drag-and-drop is active
        // (e.g. dragging a dock tab or a resize handle across TextViews).
        if cx.has_active_drag() {
            return;
        }

        // Compute the selection band before and after moving the cursor so
        // notify can be limited to the views that actually changed.
        let old_points = self.text_selection.resolved_points(cx);
        let endpoint = self.text_selection_endpoint(position, window, cx);
        self.text_selection.did_hit_text |= endpoint.inside_text;
        self.text_selection.cursor = Some(endpoint);
        let new_points = self.text_selection.resolved_points(cx);

        // Auto-scroll the anchor view when dragging near its viewport edges.
        // Only a true-hit anchor auto-scrolls; a proxy-anchored view must not.
        if let Some(view) = self
            .text_selection
            .anchor
            .as_ref()
            .filter(|e| e.inside)
            .and_then(|e| e.view.as_ref())
            .and_then(|v| v.upgrade())
        {
            view.update(cx, |state, cx| {
                if state.scrollable {
                    let delta = AutoScroll::compute_delta(position.y, state.bounds());
                    state.set_auto_scroll(delta, cx);
                }
            });
        }

        self.notify_selection_band(old_points, new_points, cx);
    }

    pub(crate) fn end_text_selection(&mut self, cx: &mut Context<Self>) {
        if !self.text_selection.is_selecting {
            return;
        }
        self.text_selection.is_selecting = false;
        if !self.text_selection.did_hit_text {
            self.text_selection.anchor = None;
            self.text_selection.cursor = None;
            return;
        }
        // Only a true-hit anchor had auto-scroll set; a proxy-anchored view
        // has nothing to tear down.
        if let Some(view) = self
            .text_selection
            .anchor
            .as_ref()
            .filter(|e| e.inside)
            .and_then(|e| e.view.as_ref())
            .and_then(|v| v.upgrade())
        {
            view.update(cx, |state, cx| {
                state.is_selecting = false;
                state.stop_auto_scroll();
                cx.notify();
            });
        }
        self.notify_selectable_text_views(cx);
    }

    /// The scope window text selection is confined to right now: the topmost
    /// open Dialog, else the active Sheet, else the base window.
    fn active_selection_scope(&self) -> SelectionScope {
        if !self.active_dialogs.is_empty() {
            SelectionScope::Dialog(self.active_dialogs.len() - 1)
        } else if self.active_sheet.is_some() {
            SelectionScope::Sheet
        } else {
            SelectionScope::Base
        }
    }

    /// Resolve a window position to a selection endpoint using hitbox hover
    /// testing; blank space proxy-anchors to the nearest view so selection follows scrolled content.
    fn text_selection_endpoint(
        &self,
        position: Point<Pixels>,
        window: &Window,
        cx: &App,
    ) -> SelectionEndpoint {
        // Confine to the active modal layer: `.occlude()` keeps true-hits off
        // behind-views, but proxy-anchor ignores occlusion, so both loops filter by scope.
        let scope = self.active_selection_scope();

        let mut best: Option<(WeakEntity<TextViewState>, f32)> = None;
        // `is_hovered` reflects the last prepaint frame (negligible lag).
        // Smallest-area wins as a proxy for the innermost view when TextViews overlap.
        for (view, hitbox, view_scope) in self.selectable_text_views.values() {
            if *view_scope != scope {
                continue;
            }
            if view.upgrade().is_none() {
                continue;
            }
            if !hitbox.is_hovered(window) {
                continue;
            }
            let area = f32::from(hitbox.bounds.size.width) * f32::from(hitbox.bounds.size.height);
            if best.as_ref().map_or(true, |(_, a)| area < *a) {
                best = Some((view.clone(), area));
            }
        }

        if let Some((view, entity)) =
            best.and_then(|(view, _)| view.upgrade().map(|entity| (view, entity)))
        {
            let state = entity.read(cx);
            let inside_text = self
                .selectable_text_inlines
                .get(&state.entity_id)
                .is_some_and(|bounds| bounds.iter().any(|bounds| bounds.contains(&position)));
            return SelectionEndpoint {
                point: position - state.bounds().origin - state.scroll_offset(),
                view: Some(view),
                inside: true,
                inside_text,
            };
        }

        // Blank space: proxy-anchor to the nearest predecessor view in document
        // flow (or the first view if above all of them) so the endpoint moves with scrolled content.
        let mut predecessor: Option<(WeakEntity<TextViewState>, Pixels)> = None;
        let mut first: Option<(WeakEntity<TextViewState>, Pixels)> = None;
        for (view, _, view_scope) in self.selectable_text_views.values() {
            if *view_scope != scope {
                continue;
            }
            let Some(entity) = view.upgrade() else {
                continue;
            };
            let top = entity.read(cx).bounds().top();
            if top <= position.y {
                if predecessor.as_ref().map_or(true, |(_, t)| top > *t) {
                    predecessor = Some((view.clone(), top));
                }
            }
            if first.as_ref().map_or(true, |(_, t)| top < *t) {
                first = Some((view.clone(), top));
            }
        }

        match predecessor.or(first) {
            Some((view, _)) => {
                let entity = view.upgrade();
                // `view.upgrade()` succeeded above when the candidate was
                // chosen; if it raced to None, fall back to a window endpoint.
                match entity {
                    Some(entity) => {
                        let state = entity.read(cx);
                        SelectionEndpoint {
                            point: position - state.bounds().origin - state.scroll_offset(),
                            view: Some(view),
                            inside: false,
                            inside_text: false,
                        }
                    }
                    None => SelectionEndpoint {
                        view: None,
                        point: position,
                        inside: false,
                        inside_text: false,
                    },
                }
            }
            None => SelectionEndpoint {
                view: None,
                point: position,
                inside: false,
                inside_text: false,
            },
        }
    }

    fn notify_selectable_text_views(&mut self, cx: &mut Context<Self>) {
        self.selectable_text_views.retain(|_, (view, _, _)| {
            let Some(view) = view.upgrade() else {
                return false;
            };
            view.update(cx, |_, cx| cx.notify());
            true
        });
    }

    /// Notify only the views affected by the current selection update: the
    /// anchor view for a single-view selection, else views intersecting the vertical band of the old and new selection.
    fn notify_selection_band(
        &mut self,
        old_points: Option<(Point<Pixels>, Point<Pixels>)>,
        new_points: Option<(Point<Pixels>, Point<Pixels>)>,
        cx: &mut Context<Self>,
    ) {
        // Single-view fast path; unsafe with a previous band, since a view
        // previously crossed may still need to clear its stale highlight.
        if old_points.is_none() {
            if let Some(id) = self.text_selection.single_view() {
                if let Some((view, _, _)) = self.selectable_text_views.get(&id) {
                    if let Some(view) = view.upgrade() {
                        view.update(cx, |_, cx| cx.notify());
                    }
                }
                return;
            }
        }

        // Merge the old (may need to clear a highlight) and new (may need to
        // paint one) selection bands; nothing to update if both are empty.
        let band = |points: Option<(Point<Pixels>, Point<Pixels>)>| {
            points.map(|(a, b)| {
                let (lo, hi) = if a.y <= b.y { (a.y, b.y) } else { (b.y, a.y) };
                (lo, hi)
            })
        };
        let (band_min, band_max) = match (band(old_points), band(new_points)) {
            (Some((lo_a, hi_a)), Some((lo_b, hi_b))) => (lo_a.min(lo_b), hi_a.max(hi_b)),
            (Some(b), None) | (None, Some(b)) => b,
            (None, None) => return,
        };

        self.selectable_text_views.retain(|_, (view, _, _)| {
            let Some(view) = view.upgrade() else {
                return false;
            };
            let bounds = view.read(cx).bounds();
            if bounds.top() <= band_max && bounds.bottom() >= band_min {
                view.update(cx, |_, cx| cx.notify());
            }
            true
        });
    }
}

/// A zero-size element that drives window-level text selection.
/// Must be the FIRST child of Root's container div, so it observes `suppress_text_selection` after press-owning components have set it.
pub(crate) struct TextSelectionController;

impl IntoElement for TextSelectionController {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextSelectionController {
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
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (window.request_layout(Style::default(), [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        _: &mut App,
    ) {
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if event.button != MouseButton::Left {
                return;
            }
            if phase.capture() {
                // Reset the suppression flag, then clear the previous
                // selection (browser behavior), regardless of bubble outcome.
                GlobalState::global_mut(cx).suppress_text_selection = false;
                Root::update(window, cx, |root, _, cx| root.clear_text_selection(cx));
            } else if event.click_count == 1 {
                // A press-owning component sets `suppress_text_selection` in
                // its own bubble handler; if set, this press is theirs.
                if GlobalState::global(cx).suppress_text_selection {
                    return;
                }
                Root::update(window, cx, |root, window, cx| {
                    root.start_text_selection(event.position, window, cx);
                });
            }
        });

        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if !phase.bubble() {
                return;
            }
            Root::update(window, cx, |root, window, cx| {
                root.update_text_selection(event.position, window, cx);
            });
        });

        window.on_mouse_event(move |_: &MouseUpEvent, phase, window, cx| {
            if !phase.bubble() {
                return;
            }
            Root::update(window, cx, |root, _, cx| root.end_text_selection(cx));
        });

        window.on_mouse_event(move |_: &ScrollWheelEvent, phase, window, cx| {
            if !phase.bubble() {
                return;
            }
            // Re-resolve the cursor endpoint at the current mouse position so a
            // wheel scroll while drag-selecting keeps extending to the pointer.
            let position = window.mouse_position();
            Root::update(window, cx, |root, window, cx| {
                root.update_text_selection(position, window, cx);
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{SelectionScope, SelectionScopeElement};
    use crate::global_state::GlobalState;
    use crate::{
        Placement, Root,
        text::{TextView, TextViewState},
    };
    use gpui::{
        AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
        Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement as _, Render,
        Styled as _, TestAppContext, VisualTestContext, Window, div, point, px,
    };
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Duration;

    struct ChatTestView {
        focus_handle: FocusHandle,
        first: Entity<TextViewState>,
        second: Entity<TextViewState>,
        second_selectable: bool,
        /// Top padding above the views; bumping it simulates an outer
        /// container scrolling (see `selection_follows_content_when_layout_shifts`).
        top_offset: gpui::Pixels,
        /// Blank gap between the two views, used to anchor a selection in blank
        /// space (the proxy-anchored endpoint path).
        mid_gap: gpui::Pixels,
    }

    impl ChatTestView {
        fn new(second_selectable: bool, cx: &mut Context<Self>) -> Self {
            Self {
                focus_handle: cx.focus_handle(),
                first: cx.new(|cx| TextViewState::markdown("Hello world", cx)),
                second: cx.new(|cx| TextViewState::markdown("Second message", cx)),
                second_selectable,
                top_offset: px(10.),
                mid_gap: px(0.),
            }
        }
    }

    impl Render for ChatTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            // `track_focus` marks every press default-prevented; selection
            // must still start from blank space here, which `suppress_text_selection` guarantees since blank-space presses never set it.
            div()
                .track_focus(&self.focus_handle)
                .size_full()
                .pt(self.top_offset)
                .child(
                    div()
                        .h(px(40.))
                        .child(TextView::new(&self.first).selectable(true)),
                )
                // A blank gap; a press here exercises the blank-space
                // (proxy-anchored) endpoint path.
                .child(div().h(self.mid_gap))
                .child(
                    div()
                        .h(px(40.))
                        .child(TextView::new(&self.second).selectable(self.second_selectable)),
                )
                // Owns its own press like Input/Button: sets the suppress
                // flag, so a press starting here must not start a selection.
                .child(
                    div()
                        .h(px(20.))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            GlobalState::suppress_text_selection(cx);
                        }),
                )
        }
    }

    fn setup(
        second_selectable: bool,
        cx: &mut TestAppContext,
    ) -> (Entity<ChatTestView>, &mut VisualTestContext) {
        cx.update(crate::init);
        let (root, cx) = cx.add_window_view(|window, cx| {
            let chat = cx.new(|cx| ChatTestView::new(second_selectable, cx));
            Root::new(chat, window, cx)
        });
        let chat = root.read_with(cx, |root, _| {
            root.view().clone().downcast::<ChatTestView>().unwrap()
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        (chat, cx)
    }

    fn drag(
        cx: &mut VisualTestContext,
        from: gpui::Point<gpui::Pixels>,
        to: gpui::Point<gpui::Pixels>,
    ) {
        drag_through(cx, &[from, to]);
    }

    fn drag_through(cx: &mut VisualTestContext, points: &[gpui::Point<gpui::Pixels>]) {
        assert!(points.len() >= 2);
        let from = points[0];
        let to = *points.last().unwrap();

        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        for point in &points[1..] {
            cx.simulate_mouse_move(*point, Some(MouseButton::Left), Modifiers::default());
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }

        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    fn window_selected_text(cx: &mut VisualTestContext) -> String {
        use crate::WindowExt as _;
        cx.update(|window, cx| window.selected_text(cx))
    }

    #[gpui::test]
    fn cross_view_drag_merges_text_top_to_bottom(cx: &mut TestAppContext) {
        let (_, cx) = setup(true, cx);

        // From the very start of the first view down into the second view.
        drag(cx, point(px(0.), px(15.)), point(px(300.), px(70.)));

        let text = window_selected_text(cx);
        let first = text.find("Hello world").expect("first view text missing");
        let second = text
            .find("Second message")
            .expect("second view text missing");
        assert!(first < second, "wrong order: {text:?}");
        assert!(text.contains('\n'), "expected newline separator: {text:?}");
    }

    #[gpui::test]
    fn drag_from_blank_space_selects_views_below(cx: &mut TestAppContext) {
        let (_, cx) = setup(true, cx);

        // Start in the blank padding above the first view, enter the second
        // view's rendered text, then drag past its end.
        drag_through(
            cx,
            &[
                point(px(5.), px(2.)),
                point(px(20.), px(70.)),
                point(px(300.), px(70.)),
            ],
        );

        let text = window_selected_text(cx);
        assert!(text.contains("Hello world"), "got: {text:?}");
        assert!(text.contains("Second message"), "got: {text:?}");
    }

    #[gpui::test]
    fn drag_entirely_in_blank_gap_selects_nothing(cx: &mut TestAppContext) {
        let (chat, cx) = setup(true, cx);

        // Layout: first [10,50], gap [50,110], second [110,150].
        chat.update(cx, |chat, cx| {
            chat.mid_gap = px(60.);
            cx.notify();
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        // Drag only inside the gap. The selection never enters either TextView.
        drag(cx, point(px(5.), px(70.)), point(px(300.), px(90.)));

        let text = window_selected_text(cx);
        assert_eq!(text, "", "blank-only drag selected text: {text:?}");
    }

    #[gpui::test]
    fn drag_entirely_in_right_gutter_selects_nothing(cx: &mut TestAppContext) {
        let (_, cx) = setup(true, cx);

        // x=300 is far to the right of the rendered text. Dragging vertically
        // through only that blank gutter must not select nearby TextViews.
        drag(cx, point(px(300.), px(2.)), point(px(300.), px(70.)));

        let text = window_selected_text(cx);
        assert_eq!(text, "", "right-gutter drag selected text: {text:?}");
    }

    #[gpui::test]
    fn selection_follows_content_when_layout_shifts(cx: &mut TestAppContext) {
        let (chat, cx) = setup(true, cx);

        // Open a blank gap so we can anchor a selection in blank space
        // below the first view's text. Layout: first [10,50], gap [50,110], second [110,150].
        chat.update(cx, |chat, cx| {
            chat.mid_gap = px(60.);
            cx.notify();
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        // Anchor in the gap and drag into the second view; anchor sits below
        // "Hello world", so only the second view is selected.
        drag_through(
            cx,
            &[
                point(px(0.), px(80.)),
                point(px(20.), px(120.)),
                point(px(300.), px(120.)),
            ],
        );
        let before = window_selected_text(cx);
        assert!(
            before.contains("Second message") && !before.contains("Hello world"),
            "expected only the second view selected, got: {before:?}"
        );

        // Shift content down 80px (simulated outer scroll). A window-anchored
        // endpoint would drift into "Hello world"; a proxy-anchored one stays stable.
        chat.update(cx, |chat, cx| {
            chat.top_offset = px(90.);
            cx.notify();
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        let after = window_selected_text(cx);
        assert_eq!(before, after, "selection drifted after layout shift");
    }

    #[gpui::test]
    fn suppressed_mouse_down_does_not_start_selection(cx: &mut TestAppContext) {
        let (_, cx) = setup(true, cx);

        // The suppress region sits below the two views (y in [90, 110)); a
        // drag starting there must not produce any window selection.
        drag(cx, point(px(20.), px(100.)), point(px(20.), px(15.)));

        let text = window_selected_text(cx);
        assert!(text.is_empty(), "expected no selection, got: {text:?}");
    }

    #[gpui::test]
    fn non_selectable_view_is_excluded(cx: &mut TestAppContext) {
        let (_, cx) = setup(false, cx);

        drag_through(
            cx,
            &[
                point(px(5.), px(2.)),
                point(px(20.), px(15.)),
                point(px(300.), px(15.)),
            ],
        );

        let text = window_selected_text(cx);
        assert!(text.contains("Hello world"), "got: {text:?}");
        assert!(!text.contains("Second message"), "got: {text:?}");
    }

    #[gpui::test]
    fn drag_within_single_view_excludes_others(cx: &mut TestAppContext) {
        let (_, cx) = setup(true, cx);

        // Entirely inside the first view.
        drag(cx, point(px(5.), px(15.)), point(px(60.), px(15.)));

        let text = window_selected_text(cx);
        assert!(!text.contains("Second message"), "got: {text:?}");
        assert!(!text.trim().is_empty(), "expected some selection");
    }

    #[gpui::test]
    fn mouse_down_clears_previous_selection(cx: &mut TestAppContext) {
        let (_, cx) = setup(true, cx);

        drag(cx, point(px(5.), px(15.)), point(px(300.), px(70.)));
        assert!(!window_selected_text(cx).is_empty());

        // A plain click clears the selection.
        cx.simulate_click(point(px(300.), px(100.)), Modifiers::default());
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        assert_eq!(window_selected_text(cx), "");
    }

    #[gpui::test]
    fn double_click_selects_word_under_root(cx: &mut TestAppContext) {
        let (_, cx) = setup(true, cx);

        // Double-click inside the first view: must trigger the per-view word
        // selection (Inline), not a window-level drag selection.
        let position = point(px(10.), px(15.));
        cx.simulate_event(MouseDownEvent {
            position,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: 2,
            first_mouse: false,
        });
        cx.simulate_event(MouseUpEvent {
            position,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: 2,
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        let text = window_selected_text(cx);
        assert_eq!(text.trim(), "Hello", "expected word selection: {text:?}");
        assert!(!text.contains("Second message"), "got: {text:?}");
    }

    #[gpui::test]
    fn drag_back_into_anchor_view_clears_other_views(cx: &mut TestAppContext) {
        let (chat, cx) = setup(true, cx);
        let second = chat.read_with(cx, |chat, _| chat.second.clone());

        // Drag from view A down into view B: this is a cross-view selection, so
        // B paints a highlight and `selected_text` reports it.
        cx.simulate_mouse_down(
            point(px(0.), px(15.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.simulate_mouse_move(
            point(px(300.), px(70.)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        let text = second.read_with(cx, |state, _| state.selected_text());
        assert!(
            text.contains("Second message"),
            "precondition: B should be selected, got {text:?}"
        );

        // A view only drops a stale highlight when notified and repainted;
        // asserts the controller does notify B.
        let b_notified = Rc::new(Cell::new(false));
        let _subscription = cx.update({
            let b_notified = b_notified.clone();
            let second = second.clone();
            move |_, cx| cx.observe(&second, move |_, _| b_notified.set(true))
        });
        b_notified.set(false);

        // Drag back into A: fast path runs but must still notify B to clear
        // its stale highlight. Checked in-drag, since mouse-up notifies every view anyway.
        cx.simulate_mouse_move(
            point(px(60.), px(15.)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        cx.run_until_parked();

        assert!(
            b_notified.get(),
            "view B was not notified when the drag returned to the anchor view, \
             so its stale highlight would never be repainted away",
        );
    }

    /// A view with a selectable TextView that also mounts the Dialog/Sheet
    /// layers, so a real modal can be opened on top of the base content.
    struct ModalScopeTestView {
        focus_handle: FocusHandle,
        base: Entity<TextViewState>,
    }

    impl ModalScopeTestView {
        fn new(cx: &mut Context<Self>) -> Self {
            Self {
                focus_handle: cx.focus_handle(),
                base: cx.new(|cx| TextViewState::markdown("Hello world", cx)),
            }
        }
    }

    impl Render for ModalScopeTestView {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let sheet_layer = Root::render_sheet_layer(window, cx);
            let dialog_layer = Root::render_dialog_layer(window, cx);
            div()
                .track_focus(&self.focus_handle)
                .size_full()
                .child(
                    div()
                        .h(px(40.))
                        .child(TextView::new(&self.base).selectable(true)),
                )
                .children(sheet_layer)
                .children(dialog_layer)
        }
    }

    fn setup_modal(
        cx: &mut TestAppContext,
    ) -> (Entity<ModalScopeTestView>, &mut VisualTestContext) {
        cx.update(crate::init);
        let (root, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(ModalScopeTestView::new);
            Root::new(view, window, cx)
        });
        let view = root.read_with(cx, |root, _| {
            root.view()
                .clone()
                .downcast::<ModalScopeTestView>()
                .unwrap()
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        (view, cx)
    }

    /// Advance past the modal open animation, then redraw so its TextViews
    /// register with stable bounds for the subsequent drag.
    fn settle(cx: &mut VisualTestContext) {
        cx.executor().advance_clock(Duration::from_millis(500));
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    fn open_dialog_with_text(
        cx: &mut VisualTestContext,
        text: &'static str,
    ) -> Entity<TextViewState> {
        let state = cx.update(|_, cx| cx.new(|cx| TextViewState::markdown(text, cx)));
        let state_for_builder = state.clone();
        cx.update(|window, cx| {
            Root::update(window, cx, |root, window, cx| {
                root.open_dialog(
                    move |dialog, _, _| {
                        dialog.child(TextView::new(&state_for_builder).selectable(true))
                    },
                    window,
                    cx,
                );
            });
        });
        settle(cx);
        state
    }

    #[gpui::test]
    fn drag_inside_dialog_still_selects_its_text(cx: &mut TestAppContext) {
        let (_, cx) = setup_modal(cx);
        let dialog_state = open_dialog_with_text(cx, "Dialog text");

        // A drag entirely within the dialog's TextView must still select (the
        // scope filter must not break in-dialog selection — see #2501).
        let b = dialog_state.read_with(cx, |s, _| s.bounds());
        drag(
            cx,
            point(b.origin.x + px(1.), b.center().y),
            point(b.origin.x + b.size.width + px(80.), b.center().y),
        );

        let text = window_selected_text(cx);
        assert!(
            text.contains("Dialog text"),
            "dialog text was not selectable: {text:?}"
        );
    }

    #[gpui::test]
    fn opening_dialog_clears_base_selection(cx: &mut TestAppContext) {
        let (view, cx) = setup_modal(cx);

        let b = view.read_with(cx, |v, cx| v.base.read(cx).bounds());
        drag(
            cx,
            point(b.origin.x + px(1.), b.center().y),
            point(b.origin.x + b.size.width + px(80.), b.center().y),
        );
        assert!(window_selected_text(cx).contains("Hello world"));

        let _dialog = open_dialog_with_text(cx, "Dialog text");

        let text = window_selected_text(cx);
        assert!(
            !text.contains("Hello world"),
            "base selection was not cleared when the dialog opened: {text:?}"
        );
    }

    /// A behind-the-modal TextView under a full-window occluding overlay,
    /// plus a modal-scoped `front` TextView painted on top, reproducing modal stacking without a real modal's open animation.
    struct SyntheticModalView {
        focus_handle: FocusHandle,
        behind: Entity<TextViewState>,
        front: Entity<TextViewState>,
        front_scope: SelectionScope,
    }

    impl SyntheticModalView {
        fn new(front_scope: SelectionScope, cx: &mut Context<Self>) -> Self {
            Self {
                focus_handle: cx.focus_handle(),
                behind: cx.new(|cx| TextViewState::markdown("Behind text", cx)),
                front: cx.new(|cx| TextViewState::markdown("Front text", cx)),
                front_scope,
            }
        }
    }

    impl Render for SyntheticModalView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .track_focus(&self.focus_handle)
                .size_full()
                // Behind the modal, at the top. Occluded by the overlay below.
                .child(
                    div()
                        .h(px(40.))
                        .child(TextView::new(&self.behind).selectable(true)),
                )
                // A full-window occluding overlay (mirrors the modal overlay)
                // with modal-scoped content painted on top of it.
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .occlude()
                        .child(
                            div()
                                .absolute()
                                .top(px(100.))
                                .left_0()
                                .h(px(40.))
                                .child(TextView::new(&self.front).selectable(true))
                                .selection_scope(self.front_scope),
                        ),
                )
        }
    }

    fn setup_synthetic(
        front_scope: SelectionScope,
        cx: &mut TestAppContext,
    ) -> (Entity<SyntheticModalView>, &mut VisualTestContext) {
        cx.update(crate::init);
        let (root, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| SyntheticModalView::new(front_scope, cx));
            Root::new(view, window, cx)
        });
        let view = root.read_with(cx, |root, _| {
            root.view()
                .clone()
                .downcast::<SyntheticModalView>()
                .unwrap()
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        (view, cx)
    }

    /// Open an empty dialog (its layer is not mounted, so nothing renders) purely
    /// to make `active_selection_scope()` return `Dialog(0)`.
    fn activate_dialog_scope(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            Root::update(window, cx, |root, window, cx| {
                root.open_dialog(|dialog, _, _| dialog, window, cx);
            });
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    /// Open an empty sheet purely to make `active_selection_scope()` return
    /// `Sheet`.
    fn activate_sheet_scope(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            Root::update(window, cx, |root, window, cx| {
                root.open_sheet_at(Placement::Right, |sheet, _, _| sheet, window, cx);
            });
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    /// Regression guard: a drag from dialog-scoped content over the overlay
    /// must not select the TextView behind it.
    #[gpui::test]
    fn selection_behind_active_dialog_is_excluded(cx: &mut TestAppContext) {
        let (view, cx) = setup_synthetic(SelectionScope::Dialog(0), cx);
        activate_dialog_scope(cx);

        // Drag onto the behind view's glyphs on the left (its center is far
        // from its text, since it spans the full window width).
        let from = view.read_with(cx, |v, cx| v.front.read(cx).bounds().center());
        let to = view.read_with(cx, |v, cx| {
            let b = v.behind.read(cx).bounds();
            point(b.origin.x + px(4.), b.center().y)
        });
        drag(cx, from, to);

        let behind = view.read_with(cx, |v, cx| v.behind.read(cx).selected_text());
        assert!(
            behind.trim().is_empty(),
            "view behind the dialog overlay was selected: {behind:?}"
        );
    }

    /// The same guard for a Sheet (#2501 de-guarded both Dialog and Sheet).
    #[gpui::test]
    fn selection_behind_active_sheet_is_excluded(cx: &mut TestAppContext) {
        let (view, cx) = setup_synthetic(SelectionScope::Sheet, cx);
        activate_sheet_scope(cx);

        let from = view.read_with(cx, |v, cx| v.front.read(cx).bounds().center());
        let to = view.read_with(cx, |v, cx| {
            let b = v.behind.read(cx).bounds();
            point(b.origin.x + px(4.), b.center().y)
        });
        drag(cx, from, to);

        let behind = view.read_with(cx, |v, cx| v.behind.read(cx).selected_text());
        assert!(
            behind.trim().is_empty(),
            "view behind the sheet overlay was selected: {behind:?}"
        );
    }

    /// The scope filter must not over-exclude: content in the active modal scope
    /// stays selectable.
    #[gpui::test]
    fn front_view_in_active_scope_is_selectable(cx: &mut TestAppContext) {
        let (view, cx) = setup_synthetic(SelectionScope::Dialog(0), cx);
        activate_dialog_scope(cx);

        let b = view.read_with(cx, |v, cx| v.front.read(cx).bounds());
        drag(
            cx,
            point(b.origin.x + px(1.), b.center().y),
            point(b.origin.x + b.size.width + px(80.), b.center().y),
        );

        let front = view.read_with(cx, |v, cx| v.front.read(cx).selected_text());
        assert!(
            front.contains("Front"),
            "active-scope content was not selectable: {front:?}"
        );
    }
}
