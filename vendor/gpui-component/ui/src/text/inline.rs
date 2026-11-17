use gpui::Corners;
use std::{
    cell::{Cell, RefCell},
    ops::Range,
    rc::Rc,
    sync::{Arc, Mutex, Weak},
};

use gpui::{
    App, BorderStyle, Bounds, CursorStyle, Edges, Element, ElementId, GlobalElementId, Half,
    HighlightStyle, Hitbox, HitboxBehavior, InspectorElementId, IntoElement, LayoutId, LineLayout,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString,
    StyledText, TextLayout, Window, WrappedLineLayout, point, px, quad,
};

use crate::{
    ActiveTheme, WindowExt as _, global_state::GlobalState, input::Selection,
    text::TextViewMultiClickKind, text::node::LinkMark, text::selection::word_range_at,
};

/// All text in TextView (including the CodeBlock) uses this for text rendering.
pub(super) struct Inline {
    id: ElementId,
    text: SharedString,
    links: Rc<Vec<(Range<usize>, LinkMark)>>,
    highlights: Arc<[(Range<usize>, HighlightStyle)]>,
    styled_text: Rc<RefCell<StyledText>>,
    cache_layout: bool,
    cached_extent: Option<gpui::Size<Pixels>>,
    extent: Option<Rc<Cell<Option<gpui::Size<Pixels>>>>>,
    cached_runs: Option<Arc<Vec<gpui::TextRun>>>,
    code_paint: Option<Rc<RefCell<CodePaintCache>>>,

    state: Arc<Mutex<InlineState>>,
}

#[derive(Debug, Default, PartialEq)]
pub(crate) struct InlineState {
    hovered_index: Option<usize>,
    /// The text actually rendered, matched against selection ranges.
    pub(super) text: SharedString,
    pub(super) selection: Option<Selection>,
}

impl InlineState {
    pub(crate) fn set_text(&mut self, text: SharedString) {
        self.text = text;
    }
}

// Layouts contain foreground-thread GPUI state, so keep them out of the Send
// parser nodes. Weak owners release entries when immutable documents disappear.
thread_local! {
    static CODE_LAYOUTS: RefCell<InlineLayouts> = RefCell::new(InlineLayouts { entries: std::collections::HashMap::new(), next_sweep: 128 });
}
struct InlineLayouts {
    entries: std::collections::HashMap<usize, CachedInline>,
    next_sweep: usize,
}
struct CachedInline {
    owner: Weak<Mutex<InlineState>>,
    text: SharedString,
    style: gpui::TextStyle,
    rem: Pixels,
    highlights: Arc<[(Range<usize>, HighlightStyle)]>,
    element: Rc<RefCell<StyledText>>,
    runs: Arc<Vec<gpui::TextRun>>,
    extent: Rc<Cell<Option<gpui::Size<Pixels>>>>,
    code_paint: Rc<RefCell<CodePaintCache>>,
}

// The full immutable layout remains the selection/caret oracle. Painting uses
// only logical lines intersecting the clip, with their original syntax runs.
struct LineSnapshot {
    lines: Arc<[Arc<WrappedLineLayout>]>,
    bytes: Vec<usize>,
    ys: Vec<Pixels>,
}
impl LineSnapshot {
    fn new(layout: &TextLayout) -> Self {
        let lines: Arc<[_]> = layout.line_layouts().into_vec().into();
        let mut bytes = Vec::with_capacity(lines.len() + 1);
        let mut ys = Vec::with_capacity(lines.len() + 1);
        bytes.push(0);
        ys.push(px(0.));
        for line in lines.iter() {
            bytes.push(bytes.last().copied().unwrap_or(0) + line.len() + 1);
            ys.push(ys.last().copied().unwrap_or(px(0.)) + line.size(layout.line_height()).height);
        }
        Self { lines, bytes, ys }
    }
    fn visible(&self, origin: Point<Pixels>, mask: Bounds<Pixels>) -> Range<usize> {
        if mask.size.width <= px(0.) || mask.size.height <= px(0.) {
            return 0..0;
        }
        let top = mask.top() - origin.y;
        let bottom = mask.bottom() - origin.y;
        let start = self.ys[1..].partition_point(|y| *y <= top);
        let end = self.ys[..self.lines.len()].partition_point(|y| *y < bottom);
        start.min(end)..end
    }
}
struct VisibleCodeLine {
    text: StyledText,
    bounds: Bounds<Pixels>,
}
struct CodePaintCache {
    snapshot: Option<Arc<LineSnapshot>>,
    runs: Arc<Vec<gpui::TextRun>>,
    run_ends: Vec<usize>,
    visible: std::collections::BTreeMap<usize, VisibleCodeLine>,
}
impl CodePaintCache {
    fn new(runs: Arc<Vec<gpui::TextRun>>) -> Self {
        let mut end = 0;
        let run_ends = runs
            .iter()
            .map(|run| {
                end += run.len;
                end
            })
            .collect();
        Self {
            snapshot: None,
            runs,
            run_ends,
            visible: Default::default(),
        }
    }
    fn prepare(
        &mut self,
        source: &str,
        full: &TextLayout,
        reshaped: bool,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if reshaped || self.snapshot.is_none() {
            self.snapshot = Some(Arc::new(LineSnapshot::new(full)));
            self.visible.clear();
        }
        let snapshot = self.snapshot.as_ref().unwrap();
        let range = snapshot.visible(
            bounds.origin,
            window.content_mask().bounds.intersect(&bounds),
        );
        self.visible.retain(|index, _| range.contains(index));
        for index in range {
            let start = snapshot.bytes[index];
            let end = (start + snapshot.lines[index].len()).min(source.len());
            let line_bounds = Bounds::new(
                bounds.origin + point(px(0.), snapshot.ys[index]),
                gpui::size(
                    bounds.size.width,
                    snapshot.ys[index + 1] - snapshot.ys[index],
                ),
            );
            let line = self.visible.entry(index).or_insert_with(|| {
                let mut clipped = Vec::new();
                let first = self.run_ends.partition_point(|end| *end <= start);
                for run_index in first..self.runs.len() {
                    let run_start = if run_index == 0 {
                        0
                    } else {
                        self.run_ends[run_index - 1]
                    };
                    if run_start >= end {
                        break;
                    }
                    let mut run = self.runs[run_index].clone();
                    run.len = self.run_ends[run_index].min(end) - run_start.max(start);
                    clipped.push(run);
                }
                let mut text = StyledText::new(source[start..end].to_owned()).with_runs(clipped);
                let (layout, _) = text.request_layout(None, None, window, cx);
                window.compute_layout(
                    layout,
                    gpui::size(
                        gpui::AvailableSpace::Definite(bounds.size.width),
                        gpui::AvailableSpace::MaxContent,
                    ),
                    cx,
                );
                VisibleCodeLine {
                    text,
                    bounds: line_bounds,
                }
            });
            line.bounds = line_bounds;
            line.text
                .prepaint(None, None, line_bounds, &mut (), window, cx);
        }
    }
    fn paint(&mut self, window: &mut Window, cx: &mut App) {
        for line in self.visible.values_mut() {
            line.text
                .paint(None, None, line.bounds, &mut (), &mut (), window, cx);
        }
    }
}

pub(super) fn release_cached_layout(state: &Arc<Mutex<InlineState>>) {
    // This is called only by the foreground TextView document owner. Parser
    // nodes are Send and may be dropped on a background thread, whose cache is
    // unrelated. try_with also permits normal application/thread teardown.
    let _ = CODE_LAYOUTS.try_with(|cache| {
        cache
            .borrow_mut()
            .entries
            .remove(&(Arc::as_ptr(state) as usize));
    });
}

impl Inline {
    pub(super) fn new(
        id: impl Into<ElementId>,
        state: Arc<Mutex<InlineState>>,
        links: Vec<(Range<usize>, LinkMark)>,
        highlights: impl Into<Arc<[(Range<usize>, HighlightStyle)]>>,
    ) -> Self {
        let text = state
            .lock()
            .map(|state| state.text.clone())
            .unwrap_or_default();

        Self {
            id: id.into(),
            links: Rc::new(links),
            highlights: highlights.into(),
            text: text.clone(),
            styled_text: Rc::new(RefCell::new(StyledText::new(text))),
            cache_layout: false,
            cached_extent: None,
            extent: None,
            cached_runs: None,
            code_paint: None,
            state,
        }
    }

    pub(super) fn cache_layout(mut self) -> Self {
        self.cache_layout = true;
        self
    }

    fn link_for_position(
        layout: &TextLayout,
        links: &Vec<(Range<usize>, LinkMark)>,
        position: Point<Pixels>,
    ) -> Option<LinkMark> {
        if links.is_empty() {
            return None;
        }
        let offset = layout.index_for_position(position).ok()?;
        for (range, link) in links.iter() {
            if range.contains(&offset) {
                return Some(link.clone());
            }
        }

        None
    }

    #[allow(unused)]
    fn paint_selected_bounds(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        window.paint_quad(gpui::PaintQuad {
            bounds,
            background: cx.theme().blue.alpha(0.01).into(),
            corner_radii: Corners::default(),
            border_color: gpui::transparent_black(),
            border_style: BorderStyle::default(),
            border_widths: gpui::Edges::all(px(0.)),
        });
    }

    fn layout_selections(
        &self,
        text_layout: &TextLayout,
        bounds: &Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> (bool, bool, Option<Selection>) {
        let Some(text_view_state) = GlobalState::global(cx).text_view_state() else {
            return (false, false, None);
        };

        let text_view_state = text_view_state.read(cx);
        let is_selectable = text_view_state.is_selectable();
        if !is_selectable {
            return (false, false, None);
        }

        if text_view_state.is_all_selected() {
            return (is_selectable, true, Some((0..self.text.len()).into()));
        }

        if let Some(selection) = text_view_state.multi_click_selection() {
            return (
                is_selectable,
                true,
                selection_for_multi_click(
                    &self.text,
                    text_layout,
                    *bounds,
                    selection.pos,
                    selection.kind,
                )
                .map(Selection::from),
            );
        }

        let Some((selection_start, selection_end)) = text_view_state.selection_points(window, cx)
        else {
            return (is_selectable, false, None);
        };
        let line_height = window.line_height();
        let mask_bounds = window.content_mask().bounds;

        // Use for debug selection bounds
        // self.paint_selected_bounds(Bounds::from_corners(selection_start, selection_end), window, cx);

        let mut selection: Option<Selection> = None;
        let mut positions = PositionCursor::new(text_layout);
        let mut offset = 0;
        let mut chars = self.text.chars().peekable();
        while let Some(c) = chars.next() {
            let Some(pos) = positions.position(offset) else {
                offset += c.len_utf8();
                continue;
            };

            let next_offset = offset + c.len_utf8();
            let mut char_width = line_height.half();
            if let Some(next_pos) = positions.position(next_offset) {
                if next_pos.y == pos.y {
                    char_width = next_pos.x - pos.x;
                }
            }

            let char_center = point(pos.x + char_width.half(), pos.y + line_height.half());
            if mask_bounds.contains(&char_center)
                && point_in_text_selection(
                    pos,
                    char_width,
                    selection_start,
                    selection_end,
                    line_height,
                )
            {
                if selection.is_none() {
                    selection = Some((offset..offset).into());
                }

                if let Some(selection) = selection.as_mut() {
                    selection.end = next_offset;
                }
            }

            offset = next_offset;
        }

        (true, true, selection)
    }

    fn paint_selection(
        selection: &Selection,
        text_layout: &TextLayout,
        bounds: &Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut App,
        color: gpui::Hsla,
    ) {
        let mut start = selection.start;
        let mut end = selection.end;
        if end < start {
            std::mem::swap(&mut start, &mut end);
        }
        let Some(start_position) = text_layout.position_for_index(start) else {
            return;
        };
        let Some(end_position) = text_layout.position_for_index(end) else {
            return;
        };

        let line_height = text_layout.line_height();
        if start_position.y == end_position.y {
            window.paint_quad(quad(
                Bounds::from_corners(
                    start_position,
                    point(end_position.x, end_position.y + line_height),
                ),
                px(0.),
                color,
                Edges::default(),
                gpui::transparent_black(),
                BorderStyle::default(),
            ));
        } else {
            window.paint_quad(quad(
                Bounds::from_corners(
                    start_position,
                    point(bounds.right(), start_position.y + line_height),
                ),
                px(0.),
                color,
                Edges::default(),
                gpui::transparent_black(),
                BorderStyle::default(),
            ));

            if end_position.y > start_position.y + line_height {
                window.paint_quad(quad(
                    Bounds::from_corners(
                        point(bounds.left(), start_position.y + line_height),
                        point(bounds.right(), end_position.y),
                    ),
                    px(0.),
                    color,
                    Edges::default(),
                    gpui::transparent_black(),
                    BorderStyle::default(),
                ));
            }

            window.paint_quad(quad(
                Bounds::from_corners(
                    point(bounds.left(), end_position.y),
                    point(end_position.x, end_position.y + line_height),
                ),
                px(0.),
                color,
                Edges::default(),
                gpui::transparent_black(),
                BorderStyle::default(),
            ));
        }
    }
}

impl IntoElement for Inline {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Inline {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_element_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let text_style = window.text_style();
        let cache_key = Arc::as_ptr(&self.state) as usize;
        // Code has no hover-dependent links. Paragraph links retain their
        // existing immediate styling path.
        if self.cache_layout {
            let cached = CODE_LAYOUTS.with(|cache| {
                cache
                    .borrow()
                    .entries
                    .get(&cache_key)
                    .filter(|old| {
                        old.owner
                            .upgrade()
                            .is_some_and(|owner| Arc::ptr_eq(&owner, &self.state))
                            && ((old.text.as_ptr() == self.text.as_ptr()
                                && old.text.len() == self.text.len())
                                || old.text == self.text)
                            && old.style == text_style
                            && old.rem == window.rem_size()
                            && (Arc::ptr_eq(&old.highlights, &self.highlights)
                                || old.highlights == self.highlights)
                    })
                    .map(|old| {
                        (
                            old.element.clone(),
                            old.runs.clone(),
                            old.extent.clone(),
                            old.code_paint.clone(),
                        )
                    })
            });
            if let Some((cached, runs, extent, code_paint)) = cached {
                if let Some(size) = extent.get() {
                    self.styled_text = cached;
                    self.code_paint = Some(code_paint);
                    self.cached_runs = Some(runs);
                    self.cached_extent = Some(size);
                    self.extent = Some(extent);
                    return (super::measured_body::request_extent(size, window), ());
                }
            }
        }

        let mut runs = Vec::new();
        let mut ix = 0;
        let hovered = self.state.lock().ok().and_then(|state| state.hovered_index);
        for (range, highlight) in self.highlights.iter() {
            let mut highlight = *highlight;
            if self.links.iter().any(|(link, _)| {
                hovered.is_some_and(|i| link.contains(&i))
                    && link.start <= range.start
                    && link.end >= range.end
            }) {
                highlight.color = Some(cx.theme().foreground);
            }
            if ix < range.start {
                runs.push(text_style.clone().to_run(range.start - ix));
            }
            runs.push(text_style.clone().highlight(highlight).to_run(range.len()));
            ix = range.end;
        }
        if ix < self.text.len() {
            runs.push(text_style.to_run(self.text.len() - ix));
        }

        let runs = Arc::new(runs);
        self.styled_text = Rc::new(RefCell::new(
            StyledText::new(self.text.clone()).with_runs(runs.as_ref().clone()),
        ));
        if self.cache_layout {
            let extent = Rc::new(Cell::new(None));
            self.extent = Some(extent.clone());
            let code_paint = Rc::new(RefCell::new(CodePaintCache::new(runs.clone())));
            self.code_paint = Some(code_paint.clone());
            CODE_LAYOUTS.with(|cache| {
                let mut cache = cache.borrow_mut();
                // Geometric sweep intervals amortize weak-owner cleanup over
                // new nodes, rather than scanning every cached node per insert.
                if cache.entries.len() >= cache.next_sweep {
                    cache.entries.retain(|_, old| old.owner.strong_count() > 0);
                    cache.next_sweep = cache.entries.len().max(64) * 2;
                }
                cache.entries.insert(
                    cache_key,
                    CachedInline {
                        owner: Arc::downgrade(&self.state),
                        text: self.text.clone(),
                        style: window.text_style(),
                        rem: window.rem_size(),
                        highlights: self.highlights.clone(),
                        element: self.styled_text.clone(),
                        runs,
                        extent,
                        code_paint,
                    },
                );
            });
        }
        let (layout_id, _) = self.styled_text.borrow_mut().request_layout(
            global_element_id,
            inspector_id,
            window,
            cx,
        );

        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let mut measured_bounds = bounds;
        let reshaped = self
            .cached_extent
            .is_some_and(|old| old.width != bounds.size.width);
        if self
            .cached_extent
            .is_some_and(|old| old.width != bounds.size.width)
        {
            // The cached full-width leaf detects both narrowing and widening.
            // Rebuild only its consumed runs for the new constraints. Matching
            // frames retain both the shaped text and the complete run list.
            if let Some(runs) = &self.cached_runs {
                let mut element = self.styled_text.borrow_mut();
                let text = std::mem::replace(&mut *element, StyledText::new(self.text.clone()));
                *element = text.with_runs(runs.as_ref().clone());
                let (layout, _) = element.request_layout(id, inspector_id, window, cx);
                window.compute_layout(
                    layout,
                    gpui::size(
                        gpui::AvailableSpace::Definite(bounds.size.width),
                        gpui::AvailableSpace::MaxContent,
                    ),
                    cx,
                );
                measured_bounds.size.height = window.layout_bounds(layout).size.height;
                if measured_bounds.size.height != bounds.size.height {
                    if let Some(owner) = GlobalState::global(cx).text_view_state().cloned() {
                        super::measured_body::request_correction(&owner, window, cx);
                    }
                }
            }
        }
        if let Some(extent) = &self.extent {
            extent.set(Some(measured_bounds.size));
        }
        self.styled_text.borrow_mut().prepaint(
            id,
            inspector_id,
            measured_bounds,
            &mut (),
            window,
            cx,
        );

        if let Some(cache) = &self.code_paint {
            let full = self.styled_text.borrow().layout().clone();
            cache
                .borrow_mut()
                .prepare(&self.text, &full, reshaped, measured_bounds, window, cx);
        }
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        hitbox
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let current_view = window.current_view();
        let hitbox = prepaint;
        let Ok(mut state) = self.state.lock() else {
            return;
        };

        let text_layout = self.styled_text.borrow().layout().clone();
        // Inline links use foreground-only hover feedback (set in request_layout).
        // A list-hover fill can obscure text when theme token palettes differ.
        if let Some(cache) = &self.code_paint {
            cache.borrow_mut().paint(window, cx);
        } else {
            self.styled_text.borrow_mut().paint(
                global_id,
                None,
                bounds,
                &mut (),
                &mut (),
                window,
                cx,
            );
        }

        // layout selections
        let (is_selectable, is_selection, selection) =
            self.layout_selections(&text_layout, &bounds, window, cx);

        state.selection = selection;

        if is_selection || is_selectable {
            window.set_cursor_style(CursorStyle::IBeam, &hitbox);
        }

        // link cursor pointer
        let mouse_position = window.mouse_position();
        if let Some(_) = Self::link_for_position(&text_layout, &self.links, mouse_position) {
            window.set_cursor_style(CursorStyle::PointingHand, &hitbox);
        }

        if let Some(selection) = &state.selection {
            Self::paint_selection(
                selection,
                &text_layout,
                &bounds,
                window,
                cx,
                cx.theme().selection,
            );
        }

        if is_selectable {
            if let Some(text_view_state) = GlobalState::global(cx).text_view_state().cloned() {
                let snapshot = self
                    .code_paint
                    .as_ref()
                    .and_then(|cache| cache.borrow().snapshot.clone());
                let text_bounds = text_line_bounds_cached(
                    &self.text,
                    &text_layout,
                    text_layout.line_height(),
                    window.content_mask().bounds,
                    snapshot.as_deref(),
                );
                crate::Root::register_selectable_text_inline(
                    &text_view_state,
                    text_bounds,
                    window,
                    cx,
                );
            }

            window.on_mouse_event({
                let hitbox = hitbox.clone();
                let text_layout = text_layout.clone();
                let inline_state = self.state.clone();
                let text = self.text.clone();
                let text_view_state = GlobalState::global(cx).text_view_state().cloned();

                move |event: &MouseDownEvent, phase, window, cx| {
                    if !phase.bubble()
                        || !hitbox.is_hovered(window)
                        || event.button != MouseButton::Left
                    {
                        return;
                    }

                    let kind = match event.click_count {
                        2 => TextViewMultiClickKind::Word,
                        3 => TextViewMultiClickKind::Paragraph,
                        _ => return,
                    };

                    let Some(range) = selection_for_multi_click(
                        &text,
                        &text_layout,
                        hitbox.bounds,
                        event.position,
                        kind,
                    ) else {
                        return;
                    };

                    let selected_text = text[range.clone()].to_string();

                    if let Ok(mut inline_state) = inline_state.lock() {
                        inline_state.selection = Some(range.into());
                    }
                    if let Some(text_view_state) = &text_view_state {
                        text_view_state.update(cx, |state, _| {
                            state.set_multi_click_selection(event.position, kind, selected_text);
                        });
                    }
                    cx.notify(current_view);
                }
            });
        }

        // Hover indices only affect links; code retains full caret geometry
        // for actual selection input without probing every line on hover.
        if !self.links.is_empty() {
            window.on_mouse_event({
                let hitbox = hitbox.clone();
                let text_layout = text_layout.clone();
                let inline_state = self.state.clone();
                move |event: &MouseMoveEvent, phase, window, cx| {
                    if !phase.bubble() {
                        return;
                    }
                    let Ok(mut state) = inline_state.lock() else {
                        return;
                    };
                    let current = state.hovered_index;
                    let updated = if hitbox.is_hovered(window) {
                        text_layout.index_for_position(event.position).ok()
                    } else {
                        None
                    };
                    //  notify update when hovering over different links
                    if current != updated {
                        state.hovered_index = updated;
                        cx.notify(current_view);
                    }
                }
            });
        }
        if !is_selection {
            window.on_mouse_event({
                let links = self.links.clone();
                let text_layout = text_layout.clone();
                let hitbox = hitbox.clone();
                let text_view_state = GlobalState::global(cx).text_view_state().cloned();

                move |event: &MouseUpEvent, phase, window, cx| {
                    if !phase.bubble() || !hitbox.is_hovered(window) {
                        return;
                    }
                    if text_view_state
                        .as_ref()
                        .is_some_and(|state| state.read(cx).has_selection(window, cx))
                    {
                        return;
                    }

                    if let Some(link) =
                        Self::link_for_position(&text_layout, &links, event.position)
                    {
                        window.end_text_selection(cx);
                        cx.stop_propagation();
                        link.open(cx);
                    }
                }
            });
        }
    }
}

/// Union each visual row's per-char boxes inside `mask_bounds`.
#[cfg(any(test, feature = "grove-test-support"))]
fn text_line_bounds(
    text: &str,
    text_layout: &TextLayout,
    line_height: Pixels,
    mask_bounds: Bounds<Pixels>,
) -> Vec<Bounds<Pixels>> {
    text_line_bounds_cached(text, text_layout, line_height, mask_bounds, None)
}
fn text_line_bounds_cached(
    text: &str,
    text_layout: &TextLayout,
    line_height: Pixels,
    mask_bounds: Bounds<Pixels>,
    snapshot: Option<&LineSnapshot>,
) -> Vec<Bounds<Pixels>> {
    let mut line_bounds = Vec::new();
    let mut current_line_y = None;
    let mut current_bounds: Option<Bounds<Pixels>> = None;
    let start = match snapshot {
        Some(snapshot) => PositionCursor::visible_from_snapshot(text_layout, mask_bounds, snapshot),
        None => PositionCursor::visible_start(text_layout, mask_bounds),
    };
    let Some((mut positions, mut offset)) = start else {
        return line_bounds;
    };

    for c in text[offset..].chars() {
        let next_offset = offset + c.len_utf8();
        let Some(pos) = positions.position(offset) else {
            offset = next_offset;
            continue;
        };

        // Positions are monotonic vertically; text below the clip cannot
        // contribute a hit rectangle. Do not revisit the rest of a huge log.
        if pos.y >= mask_bounds.bottom() {
            break;
        }
        let mut char_width = line_height.half();
        if let Some(next_pos) = positions.position(next_offset) {
            if next_pos.y == pos.y {
                char_width = next_pos.x - pos.x;
            }
        }

        let bounds = Bounds::from_corners(pos, point(pos.x + char_width, pos.y + line_height))
            .intersect(&mask_bounds);
        if bounds.size.width > px(0.) && bounds.size.height > px(0.) {
            if current_line_y == Some(pos.y) {
                if let Some(current) = current_bounds.as_mut() {
                    *current = current.union(&bounds);
                }
            } else {
                if let Some(current) = current_bounds.take() {
                    line_bounds.push(current);
                }
                current_line_y = Some(pos.y);
                current_bounds = Some(bounds);
            }
        }

        offset = next_offset;
    }

    if let Some(current) = current_bounds {
        line_bounds.push(current);
    }

    line_bounds
}

/// Test-support oracle for a real platform TextLayout. The reference uses
/// GPUI's native caret lookup for every character, without the clipping seek.
#[cfg(feature = "grove-test-support")]
pub fn verify_text_hit_bounds_against_reference(
    text: &str,
    layout: &TextLayout,
    mask: Bounds<Pixels>,
) -> Result<(), String> {
    let line_height = layout.line_height();
    let mut expected = Vec::new();
    let mut current_y = None;
    let mut current: Option<Bounds<Pixels>> = None;
    let mut offset = 0;
    for ch in text.chars() {
        let next = offset + ch.len_utf8();
        if let Some(position) = layout.position_for_index(offset) {
            let width = layout
                .position_for_index(next)
                .filter(|next| next.y == position.y)
                .map_or(line_height.half(), |next| next.x - position.x);
            let bounds = Bounds::from_corners(
                position,
                point(position.x + width, position.y + line_height),
            )
            .intersect(&mask);
            if bounds.size.width > px(0.) && bounds.size.height > px(0.) {
                if current_y == Some(position.y) {
                    if let Some(current) = current.as_mut() {
                        *current = current.union(&bounds);
                    }
                } else {
                    if let Some(current) = current.take() {
                        expected.push(current);
                    }
                    current_y = Some(position.y);
                    current = Some(bounds);
                }
            }
        }
        offset = next;
    }
    if let Some(current) = current {
        expected.push(current);
    }
    let actual = text_line_bounds(text, layout, line_height, mask);
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "native hit geometry mismatch: actual={actual:?}; expected={expected:?}"
        ))
    }
}

/// `TextLayout::position_for_index` for non-decreasing indices in amortized O(1).
///
/// The layout's lookup rescans every line and glyph before the index, so calling it per char
/// is quadratic: a pasted 52K-char log line took seconds per frame.
struct PositionCursor {
    lines: Arc<[Arc<WrappedLineLayout>]>,
    line_height: Pixels,
    line: usize,
    line_start: usize,
    line_origin: Point<Pixels>,
    row: usize,
    row_start_x: Option<Pixels>,
    /// (run, glyph) of the first glyph whose index reached the last query.
    glyph: (usize, usize),
}

impl PositionCursor {
    fn new(text_layout: &TextLayout) -> Self {
        Self {
            lines: text_layout.line_layouts().into_vec().into(),
            line_height: text_layout.line_height(),
            line: 0,
            line_start: 0,
            line_origin: text_layout.bounds().origin,
            row: 0,
            row_start_x: None,
            glyph: (0, 0),
        }
    }

    /// Start immediately before the first visible wrapped row. Wrap-boundary
    /// glyphs provide a byte/glyph cursor without scanning the hidden prefix.
    /// Keep a preceding row because caret positions at an exact wrap boundary
    /// belong to the previous row in GPUI's inclusive index convention.
    fn visible_start(text_layout: &TextLayout, mask: Bounds<Pixels>) -> Option<(Self, usize)> {
        let line_height = text_layout.line_height();
        if line_height <= px(0.) || mask.size.width <= px(0.) || mask.size.height <= px(0.) {
            return None;
        }
        let snapshot = LineSnapshot::new(text_layout);
        Self::visible_from_snapshot(text_layout, mask, &snapshot)
    }
    fn visible_from_snapshot(
        text_layout: &TextLayout,
        mask: Bounds<Pixels>,
        snapshot: &LineSnapshot,
    ) -> Option<(Self, usize)> {
        let line_height = text_layout.line_height();
        if line_height <= px(0.) {
            return None;
        }
        let origin = text_layout.bounds().origin;
        let line_index = snapshot.visible(origin, mask).next()?;
        let line = &snapshot.lines[line_index];
        let line_origin = origin + point(px(0.), snapshot.ys[line_index]);
        let line_start = snapshot.bytes[line_index];
        let visible_row = ((mask.top() - line_origin.y).max(px(0.)) / line_height).floor() as usize;
        let start_row = visible_row
            .saturating_sub(1)
            .min(line.wrap_boundaries.len());
        let start_boundary = start_row
            .checked_sub(1)
            .map(|row| &line.wrap_boundaries[row]);
        let start_byte = start_boundary.map_or(0, |boundary| {
            line.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index
        });
        let row = start_row.saturating_sub(1);
        let glyph = row.checked_sub(1).map_or((0, 0), |row| {
            let boundary = &line.wrap_boundaries[row];
            (boundary.run_ix, boundary.glyph_ix)
        });
        Some((
            Self {
                lines: snapshot.lines.clone(),
                line_height,
                line: line_index,
                line_start,
                line_origin,
                row,
                row_start_x: None,
                glyph,
            },
            line_start + start_byte,
        ))
    }

    fn position(&mut self, index: usize) -> Option<Point<Pixels>> {
        let line = loop {
            let line = self.lines.get(self.line)?.clone();
            let line_end = self.line_start + line.len();
            if index <= line_end {
                break line;
            }
            self.line_origin.y += line.size(self.line_height).height;
            self.line_start = line_end + 1;
            self.line += 1;
            self.row = 0;
            self.row_start_x = None;
            self.glyph = (0, 0);
        };

        let index = index - self.line_start;
        let row_end = |row: usize| {
            let boundary = &line.wrap_boundaries[row];
            line.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index
        };
        while self.row < line.wrap_boundaries.len() && index > row_end(self.row) {
            self.row += 1;
            self.row_start_x = None;
        }
        let row_start_x = match self.row_start_x {
            Some(x) => x,
            None => {
                let row_start = self.row.checked_sub(1).map_or(0, row_end);
                let x = self.x_for_index(&line.unwrapped_layout, row_start);
                *self.row_start_x.insert(x)
            }
        };
        let x = self.x_for_index(&line.unwrapped_layout, index) - row_start_x;
        Some(self.line_origin + point(x, self.row as f32 * self.line_height))
    }

    /// `LineLayout::x_for_index`, resuming from the previous query's glyph.
    fn x_for_index(&mut self, layout: &LineLayout, index: usize) -> Pixels {
        let (mut run, mut glyph) = self.glyph;
        while let Some(shaped) = layout.runs.get(run) {
            match shaped.glyphs.get(glyph) {
                Some(found) if found.index >= index => {
                    self.glyph = (run, glyph);
                    return found.position.x;
                }
                Some(_) => glyph += 1,
                None => (run, glyph) = (run + 1, 0),
            }
        }
        self.glyph = (run, glyph);
        layout.width
    }
}

fn selection_for_multi_click(
    text: &str,
    text_layout: &TextLayout,
    bounds: Bounds<Pixels>,
    pos: Point<Pixels>,
    kind: TextViewMultiClickKind,
) -> Option<std::ops::Range<usize>> {
    if !bounds.contains(&pos) {
        return None;
    }

    let offset = text_layout.index_for_position(pos).ok()?;

    match kind {
        TextViewMultiClickKind::Word => word_range_at(text, offset),
        // Known limitation: a paragraph with an inline image splits into multiple runs; triple-click only selects one side of it.
        TextViewMultiClickKind::Paragraph => (!text.is_empty()).then_some(0..text.len()),
    }
}

/// Check if a `pos` is within a `bounds`, considering multi-line selections.
fn point_in_text_selection(
    pos: Point<Pixels>,
    char_width: Pixels,
    selection_start: Point<Pixels>,
    selection_end: Point<Pixels>,
    line_height: Pixels,
) -> bool {
    let point_in_line = |point: Point<Pixels>| point.y >= pos.y && point.y < pos.y + line_height;
    let top = selection_start.y.min(selection_end.y);
    let bottom = selection_start.y.max(selection_end.y);
    let x = pos.x + char_width.half();

    // Out of the vertical bounds
    if pos.y + line_height <= top || pos.y > bottom {
        return false;
    }

    // Treat the selection as single-line when both drag points fall within the same rendered line, even if their y coordinates differ inside that line.
    if point_in_line(selection_start) && point_in_line(selection_end) {
        let left = selection_start.x.min(selection_end.x);
        let right = selection_start.x.max(selection_end.x);
        return x >= left && x <= right;
    }

    let (top_point, bottom_point) = if selection_start.y < selection_end.y {
        (selection_start, selection_end)
    } else {
        (selection_end, selection_start)
    };
    let is_top_line = point_in_line(top_point);
    let is_bottom_line = point_in_line(bottom_point);

    if is_top_line {
        return x >= top_point.x;
    } else if is_bottom_line {
        return x <= bottom_point.x;
    } else {
        return true;
    }
}

#[cfg(test)]
mod tests {
    use super::{PositionCursor, point_in_text_selection, text_line_bounds};
    use gpui::{
        Bounds, IntoElement, ParentElement, Pixels, StyledText, TestAppContext, TextLayout, div,
        point, px, size,
    };

    fn laid_out(text: &str, width: Pixels, cx: &mut TestAppContext) -> TextLayout {
        let styled = StyledText::new(text.to_owned());
        let layout = styled.layout().clone();
        let cx = cx.add_empty_window();
        cx.draw(point(px(0.), px(0.)), size(width, px(100_000.)), |_, _| {
            gpui::Styled::w(div(), width)
                .child(styled)
                .into_any_element()
        });
        layout
    }

    /// The per-char scan this file used before `PositionCursor`, kept as the expected output.
    fn reference_line_bounds(
        text: &str,
        text_layout: &TextLayout,
        line_height: Pixels,
        mask_bounds: Bounds<Pixels>,
    ) -> Vec<Bounds<Pixels>> {
        let mut line_bounds = Vec::new();
        let mut current_line_y = None;
        let mut current_bounds: Option<Bounds<Pixels>> = None;
        let mut offset = 0;
        for c in text.chars() {
            let next_offset = offset + c.len_utf8();
            let Some(pos) = text_layout.position_for_index(offset) else {
                offset = next_offset;
                continue;
            };
            let mut char_width = line_height / 2.;
            if let Some(next_pos) = text_layout.position_for_index(next_offset) {
                if next_pos.y == pos.y {
                    char_width = next_pos.x - pos.x;
                }
            }
            let bounds = Bounds::from_corners(pos, point(pos.x + char_width, pos.y + line_height))
                .intersect(&mask_bounds);
            if bounds.size.width > px(0.) && bounds.size.height > px(0.) {
                if current_line_y == Some(pos.y) {
                    if let Some(current) = current_bounds.as_mut() {
                        *current = current.union(&bounds);
                    }
                } else {
                    if let Some(current) = current_bounds.take() {
                        line_bounds.push(current);
                    }
                    current_line_y = Some(pos.y);
                    current_bounds = Some(bounds);
                }
            }
            offset = next_offset;
        }
        if let Some(current) = current_bounds {
            line_bounds.push(current);
        }
        line_bounds
    }

    #[gpui::test]
    fn replacing_code_document_releases_cached_text_layout(cx: &mut TestAppContext) {
        use super::CODE_LAYOUTS;
        use crate::text::{TextView, TextViewState};
        use gpui::AppContext as _;
        cx.update(crate::init);
        let owner = cx.update(|cx| {
            cx.new(|cx| TextViewState::markdown("```text\nlarge retained code\n```", cx))
        });
        let cx = cx.add_empty_window();
        cx.draw(point(px(0.), px(0.)), size(px(480.), px(800.)), |_, _| {
            TextView::new(&owner)
        });
        let key = owner.read_with(cx, |owner, _| owner.code_blocks()[0].render_state_key());
        assert!(CODE_LAYOUTS.with(|cache| cache.borrow().entries.contains_key(&key)));
        // Replacement occurs before another frame or cache pressure. Old code
        // must release its full retained text and shaped glyphs immediately.
        cx.update(|_, cx| owner.update(cx, |owner, cx| owner.set_text("plain replacement", cx)));
        assert!(!CODE_LAYOUTS.with(|cache| cache.borrow().entries.contains_key(&key)));
    }

    #[gpui::test]
    fn cached_code_resize_matches_fresh_highlighted_text(cx: &mut TestAppContext) {
        use super::{CODE_LAYOUTS, Inline, InlineState};
        use gpui::{
            Context, FontWeight, HighlightStyle, Render, SharedString, Styled as _, Window,
        };
        use std::sync::{Arc, Mutex};

        cx.update(crate::init);
        let text: SharedString = "let café = repeated_identifier + another_identifier;\n    println!(\"wide and narrow code remains highlighted\");\n".repeat(6).into();
        let highlight = HighlightStyle {
            color: Some(gpui::rgb(0xd12255).into()),
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        let highlights: Arc<[_]> = vec![(0..3, highlight), (4..9, highlight)].into();
        let state = Arc::new(Mutex::new(InlineState::default()));
        state.lock().unwrap().set_text(text.clone());
        let key = Arc::as_ptr(&state) as usize;
        struct CodeResizeRoot {
            state: Arc<Mutex<InlineState>>,
            highlights: Arc<[(std::ops::Range<usize>, HighlightStyle)]>,
            text: SharedString,
            width: Pixels,
            fresh: Option<TextLayout>,
        }
        impl Render for CodeResizeRoot {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let oracle = StyledText::new(self.text.clone())
                    .with_highlights(self.highlights.iter().cloned());
                self.fresh = Some(oracle.layout().clone());
                div()
                    .w(self.width)
                    .child(
                        Inline::new(
                            "cached-code",
                            self.state.clone(),
                            vec![],
                            self.highlights.clone(),
                        )
                        .cache_layout(),
                    )
                    .child(oracle)
            }
        }
        let (view, cx) = cx.add_window_view(|_, _| CodeResizeRoot {
            state: state.clone(),
            highlights: highlights.clone(),
            text: text.clone(),
            width: px(480.),
            fresh: None,
        });
        let mut narrow_height = None;
        let mut wide_height = None;
        // Resize the container while the window stays fixed: this catches pane
        // resizing that a viewport-width-only cache key cannot observe.
        for width in [480., 130., 130., 620., 100., 480.] {
            let width = px(width);
            for _ in 0..2 {
                cx.update(|window, cx| {
                    view.update(cx, |view, cx| {
                        view.width = width;
                        cx.notify();
                    });
                    let _ = window.draw(cx);
                });
            }
            let (cached, syntax_runs, viewport_paint) = CODE_LAYOUTS.with(|cache| {
                let cache = cache.borrow();
                let entry = &cache.entries[&key];
                let layout = entry.element.borrow().layout().clone();
                (layout, entry.runs.clone(), entry.code_paint.clone())
            });
            let fresh = view.read_with(cx, |view, _| view.fresh.clone().unwrap());
            assert_eq!(cached.line_height(), fresh.line_height());
            assert_eq!(
                cached.bounds().size.height,
                fresh.bounds().size.height,
                "height after resizing to {width:?}"
            );
            for index in text
                .char_indices()
                .map(|(index, _)| index)
                .chain(std::iter::once(text.len()))
            {
                assert_eq!(
                    cached
                        .position_for_index(index)
                        .map(|point| point - cached.bounds().origin),
                    fresh
                        .position_for_index(index)
                        .map(|point| point - fresh.bounds().origin),
                    "caret geometry at {index}, width {width:?}"
                );
            }
            {
                let paint = viewport_paint.borrow();
                let snapshot = paint.snapshot.as_ref().unwrap();
                assert!(!paint.visible.is_empty());
                for (&index, line) in &paint.visible {
                    let start = snapshot.bytes[index];
                    let end = start + snapshot.lines[index].len();
                    for local in text[start..end]
                        .char_indices()
                        .map(|(local, _)| local)
                        .chain(std::iter::once(end - start))
                    {
                        assert_eq!(
                            line.text.layout().position_for_index(local),
                            cached.position_for_index(start + local),
                            "painted line {index} must retain full-layout caret geometry at width {width:?}"
                        );
                    }
                }
            }
            assert_eq!(syntax_runs[0].len, 3);
            assert_eq!(syntax_runs[0].color, highlight.color.unwrap());
            assert_eq!(syntax_runs[0].font.weight, FontWeight::BOLD);
            assert_eq!(
                syntax_runs.iter().map(|run| run.len).sum::<usize>(),
                text.len()
            );
            if width == px(100.) {
                narrow_height = Some(cached.bounds().size.height);
            }
            if width == px(620.) {
                wide_height = Some(cached.bounds().size.height);
            }
        }
        assert!(
            narrow_height.unwrap() > wide_height.unwrap(),
            "fixture must actually rewrap"
        );
    }

    #[gpui::test]
    fn position_cursor_matches_layout_lookup(cx: &mut TestAppContext) {
        let text = "first wrapped paragraph line with several words\n\nü naïve — wraps again here too\nlast";
        let layout = laid_out(text, px(90.), cx);
        assert!(
            layout
                .line_layouts()
                .iter()
                .any(|line| !line.wrap_boundaries.is_empty())
        );
        let mut cursor = PositionCursor::new(&layout);
        for index in (0..=text.len() + 2)
            .filter(|index| *index > text.len() || text.is_char_boundary(*index))
        {
            assert_eq!(
                cursor.position(index),
                layout.position_for_index(index),
                "index {index}"
            );
        }
    }

    #[gpui::test]
    fn line_bounds_match_per_char_scan(cx: &mut TestAppContext) {
        let text = "first wrapped paragraph line with several words\n\nü naïve — wraps again here too\nlast\n";
        let layout = laid_out(text, px(90.), cx);
        let line_height = layout.line_height();
        let everything = Bounds::new(point(px(-10.), px(-10.)), size(px(1000.), px(1000.)));
        let clipped = Bounds::new(
            point(px(20.), line_height * 1.5),
            size(px(40.), line_height * 3.),
        );
        for mask in [everything, clipped] {
            assert_eq!(
                text_line_bounds(text, &layout, line_height, mask),
                reference_line_bounds(text, &layout, line_height, mask)
            );
        }
    }

    #[gpui::test]
    fn code_line_snapshot_matches_reference_at_a_distant_viewport(cx: &mut TestAppContext) {
        let text = "naïve café 界🙂 repeated words wrap correctly here\n\n".repeat(150);
        let layout = laid_out(&text, px(170.), cx);
        let snapshot = super::LineSnapshot::new(&layout);
        let height = layout.line_height();
        let mask = Bounds::new(point(px(0.), height * 275.), size(px(170.), height * 3.));
        assert_eq!(
            super::text_line_bounds_cached(&text, &layout, height, mask, Some(&snapshot)),
            reference_line_bounds(&text, &layout, height, mask)
        );
        let visible = snapshot.visible(layout.bounds().origin, mask);
        assert!(
            visible.start > 100,
            "fixture must seek past a long hidden prefix"
        );
        assert!(
            visible.len() <= 5,
            "logical line cache must stay viewport bounded"
        );
    }

    #[gpui::test]
    fn clipped_bounds_match_reference_after_skipping_wrapped_unicode_prefix(
        cx: &mut TestAppContext,
    ) {
        let text = "naïve café 界🙂 e\u{301} 👩‍👩‍👧‍👦 office ffi\tשלום مرحبا words wrap safely here "
            .repeat(10)
            + "\n\nlast naïve line\n\t    indentation\n";
        let layout = laid_out(&text, px(170.), cx);
        let line_height = layout.line_height();
        for row in [0., 1., 2.5, 7., 12.5, 20., 35., 60., 200.] {
            for x in [0., 25.] {
                let mask = Bounds::new(
                    point(px(x), line_height * row),
                    size(px(110.), line_height * 2.25),
                );
                assert_eq!(
                    text_line_bounds(&text, &layout, line_height, mask),
                    reference_line_bounds(&text, &layout, line_height, mask),
                    "visible rows beginning at {row}, horizontal clip {x}"
                );
            }
        }
    }

    #[gpui::test]
    fn long_paragraph_line_bounds_stay_linear(cx: &mut TestAppContext) {
        // A pasted crash report: one 52K-char line inside a 95K-char message.
        let text = format!(
            "{}\n{}",
            "0x1a2b3c ".repeat(5_800),
            "frame line\n".repeat(4_000)
        );
        let layout = laid_out(&text, px(420.), cx);
        let mask = Bounds::new(point(px(0.), px(0.)), size(px(420.), px(800.)));
        let started = std::time::Instant::now();
        let bounds = text_line_bounds(&text, &layout, layout.line_height(), mask);
        let elapsed = started.elapsed();
        assert!(!bounds.is_empty());
        assert!(
            elapsed < std::time::Duration::from_millis(250),
            "took {elapsed:?}"
        );
    }

    #[test]
    fn test_point_in_text_selection() {
        let line_height = px(20.);
        let char_width = px(10.);
        let start = point(px(50.), px(50.));
        let end = point(px(150.), px(150.));

        // First line but half line height, true.
        assert!(point_in_text_selection(
            point(px(50.), px(40.)),
            char_width,
            start,
            end,
            line_height
        ));

        // First line in selection, true.
        assert!(point_in_text_selection(
            point(px(50.), px(50.)),
            char_width,
            start,
            end,
            line_height
        ));
        // First line, but left out of selection, false.
        assert!(!point_in_text_selection(
            point(px(40.), px(50.)),
            char_width,
            start,
            end,
            line_height
        ));
        // First line but right out of selection, true.
        assert!(point_in_text_selection(
            point(px(160.), px(50.)),
            char_width,
            start,
            end,
            line_height
        ));

        // Middle line in selection, true.
        assert!(point_in_text_selection(
            point(px(100.), px(70.)),
            char_width,
            start,
            end,
            line_height
        ));
        // Middle line, but left out of selection, true.
        assert!(point_in_text_selection(
            point(px(40.), px(70.)),
            char_width,
            start,
            end,
            line_height
        ));
        // Middle line, but right out of selection, true.
        assert!(point_in_text_selection(
            point(px(160.), px(70.)),
            char_width,
            start,
            end,
            line_height
        ));

        // Last line in selection, true |-----------| | selection | |------- p -|
        assert!(point_in_text_selection(
            point(px(100.), px(140.)),
            char_width,
            start,
            end,
            line_height
        ));
        // Last line, but left out of selection, true.
        assert!(point_in_text_selection(
            point(px(40.), px(140.)),
            char_width,
            start,
            end,
            line_height
        ));
        // Last line, but right out of selection, false.
        assert!(!point_in_text_selection(
            point(px(160.), px(140.)),
            char_width,
            start,
            end,
            line_height
        ));

        // Out of vertical bounds (top), false p |-----------| | selection | |-----------|
        assert!(!point_in_text_selection(
            point(px(100.), px(20.)),
            char_width,
            start,
            end,
            line_height
        ));
        // Out of vertical bounds (bottom), false |-----------| | selection | |-----------| p
        assert!(!point_in_text_selection(
            point(px(100.), px(160.)),
            char_width,
            start,
            end,
            line_height
        ));
    }

    #[test]
    fn test_point_in_text_selection_reversed_drag_direction() {
        let line_height = px(20.);
        let char_width = px(10.);

        // Mouse down on lower line then drag upward to x=150. Top line should follow current mouse x, bottom line should keep anchor x.
        let start = point(px(80.), px(150.));
        let end = point(px(150.), px(50.));

        // On top line, selection starts from top cursor x (150), so x=140 should be excluded.
        assert!(!point_in_text_selection(
            point(px(140.), px(50.)),
            char_width,
            start,
            end,
            line_height
        ));
        assert!(point_in_text_selection(
            point(px(150.), px(50.)),
            char_width,
            start,
            end,
            line_height
        ));

        // On bottom line, selection ends at anchor x (80), so x=90 should be excluded.
        assert!(point_in_text_selection(
            point(px(75.), px(140.)),
            char_width,
            start,
            end,
            line_height
        ));
        assert!(!point_in_text_selection(
            point(px(80.), px(140.)),
            char_width,
            start,
            end,
            line_height
        ));
    }

    #[test]
    fn test_point_in_text_selection_same_visual_line_with_different_y() {
        let line_height = px(20.);
        let char_width = px(10.);
        let start = point(px(100.), px(55.));
        let end = point(px(60.), px(58.));

        assert!(!point_in_text_selection(
            point(px(40.), px(50.)),
            char_width,
            start,
            end,
            line_height
        ));
        assert!(point_in_text_selection(
            point(px(70.), px(50.)),
            char_width,
            start,
            end,
            line_height
        ));
        assert!(!point_in_text_selection(
            point(px(110.), px(50.)),
            char_width,
            start,
            end,
            line_height
        ));
    }

    #[test]
    fn test_point_in_text_selection_same_visual_line_with_reversed_y() {
        let line_height = px(20.);
        let char_width = px(10.);
        let start = point(px(60.), px(58.));
        let end = point(px(100.), px(55.));

        assert!(!point_in_text_selection(
            point(px(40.), px(50.)),
            char_width,
            start,
            end,
            line_height
        ));
        assert!(point_in_text_selection(
            point(px(70.), px(50.)),
            char_width,
            start,
            end,
            line_height
        ));
        assert!(!point_in_text_selection(
            point(px(110.), px(50.)),
            char_width,
            start,
            end,
            line_height
        ));
    }
}
