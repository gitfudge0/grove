//! Keep physical scroll children stable while materializing static message rows
//! only when they intersect the viewport. Cold layout still measures every row.
use super::*;
use gpui::{
    AnyElement, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, LayoutId, Pixels,
    Size, TextStyle, WeakEntity,
};
use std::cell::RefCell;

pub(super) struct MessageFrame {
    pub state: Rc<codex::Snapshot>,
    pub prepared: Rc<PreparedTranscript>,
    pub initial_transcript: bool,
    pub near_bottom: bool,
    pub elapsed: f32,
}

#[derive(Default)]
pub(super) struct Measurement {
    source: Option<codex::TranscriptItem>,
    size: Option<Size<Pixels>>,
    text_style: Option<TextStyle>,
    rem: Pixels,
    viewport_width: Pixels,
    owner: Option<WeakEntity<TextViewState>>,
    owner_bounds: Bounds<Pixels>,
    generation: usize,
    group_key: Option<String>,
    focus: Option<FocusHandle>,
    hover_key: String,
    controls: Vec<ControlTarget>,
    pending: bool,
}

#[cfg(test)]
impl Measurement {
    pub(super) fn retains_source_for_test(&self) -> bool {
        self.source.is_some()
    }
}

pub(super) struct MeasuredTranscriptSlot {
    view: WeakEntity<CodexView>,
    index: usize,
    frame: Rc<MessageFrame>,
    owner_key: String,
    cache: Rc<RefCell<Measurement>>,
}

pub(super) struct SlotLayout {
    element: Option<AnyElement>,
    owner: Option<Entity<TextViewState>>,
    projection: Option<gpui_component::text::TextViewProjection>,
    proxy: Option<FocusProxy>,
    controls: Vec<AnyElement>,
    cached_size: Option<Size<Pixels>>,
    eager: bool,
}
#[derive(Clone, Copy)]
enum UserControl {
    Edit,
    Copy,
}
#[derive(Clone)]
struct ControlTarget {
    focus: FocusHandle,
    source_key: String,
    kind: UserControl,
}

/// A dispatch node for one original control, sharing the slot's already computed
/// layout. Each actual Tab destination keeps its own node and original handle.
struct ControlAlias {
    layout: LayoutId,
    proxy: FocusProxy,
}
impl IntoElement for ControlAlias {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for ControlAlias {
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
        bounds: Bounds<Pixels>,
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.proxy.prepaint(bounds, window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (): &mut (),
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.proxy.paint(bounds, window, cx);
    }
}
fn user_control_proxy(
    layout: LayoutId,
    target: &ControlTarget,
    view: WeakEntity<CodexView>,
    index: usize,
    item: codex::TranscriptItem,
) -> AnyElement {
    let suffix = match target.kind {
        UserControl::Edit => "edit",
        UserControl::Copy => "copy",
    };
    let id = ElementId::from(gpui::SharedString::from(format!(
        "{}:{suffix}-proxy",
        target.source_key
    )));
    let key = target.source_key.clone();
    let kind = target.kind;
    let mut div = div()
        .id(id.clone())
        .track_focus(&target.focus)
        .tab_index(0)
        .role(gpui::Role::Button)
        .aria_label(match kind {
            UserControl::Edit => "Edit sent message",
            UserControl::Copy => "Copy message",
        })
        .on_key_down(move |event, window, cx| {
            if action_key(event) {
                let _ = view.update(cx, |view, cx| match kind {
                    UserControl::Edit => view.request_edit(index, window, cx),
                    UserControl::Copy => view.copy_message(
                        key.clone(),
                        reference_prompt_copy(&codex::item_text(item.as_ref())),
                        cx,
                    ),
                });
                cx.stop_propagation();
            }
        });
    ControlAlias {
        layout,
        proxy: FocusProxy {
            id,
            interactivity: std::mem::take(div.interactivity()),
            hitbox: None,
        },
    }
    .into_any_element()
}

/// Reuse GPUI's focus, accessibility, tab-stop and hover registration on the
/// slot's dispatch node. Geometry is already known; another Taffy root would
/// only repeat work for an otherwise empty offscreen source target.
struct FocusProxy {
    id: ElementId,
    interactivity: gpui::Interactivity,
    hitbox: Option<gpui::Hitbox>,
}
impl FocusProxy {
    fn prepaint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        self.hitbox = window.with_global_id(self.id.clone(), |id, window| {
            self.interactivity.prepaint(
                Some(id),
                None,
                bounds,
                bounds.size,
                window,
                cx,
                |_, _, hitbox, _, _| hitbox,
            )
        });
    }
    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        window.with_global_id(self.id.clone(), |id, window| {
            self.interactivity.paint(
                Some(id),
                None,
                bounds,
                self.hitbox.as_ref(),
                window,
                cx,
                |_, _, _| {},
            );
        });
    }
}

impl MeasuredTranscriptSlot {
    pub fn new(
        view: WeakEntity<CodexView>,
        index: usize,
        frame: Rc<MessageFrame>,
        owner_key: String,
        cache: Rc<RefCell<Measurement>>,
    ) -> Self {
        Self {
            view,
            index,
            frame,
            owner_key,
            cache,
        }
    }
    fn materialize(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> (AnyElement, Option<Entity<TextViewState>>) {
        #[cfg(test)]
        let _phase = phases::Section::materialize();
        #[cfg(test)]
        MATERIALIZED.with(|count| count.set(count.get() + 1));
        self.view
            .update(cx, |view, cx| {
                let row = view.render_message_row(self.index, &self.frame, window, cx);
                let owner = view.selection_views.get(&self.owner_key).cloned();
                let item = &self.frame.state.saved.items[self.index];
                let id = item["id"]
                    .as_str()
                    .map_or_else(|| self.index.to_string(), str::to_owned);
                let user = item["type"] == "userMessage";
                let group = self.frame.prepared.response_prose[self.index]
                    .map(|i| &self.frame.prepared.response_groups[i]);
                let key = if user {
                    format!(
                        "{}:{id}",
                        self.frame
                            .state
                            .saved
                            .thread_id
                            .as_deref()
                            .unwrap_or_default()
                    )
                } else {
                    group.map_or_else(String::new, |group| format!("{}:source:{id}", group.key))
                };
                let mut cache = self.cache.borrow_mut();
                cache.focus = view.message_focus.get(&key).cloned();
                cache.controls.clear();
                if user {
                    for (suffix, kind) in [("edit", UserControl::Edit), ("copy", UserControl::Copy)]
                    {
                        if let Some(focus) = view.message_focus.get(&format!("{key}:{suffix}")) {
                            cache.controls.push(ControlTarget {
                                focus: focus.clone(),
                                source_key: key.clone(),
                                kind,
                            });
                        }
                    }
                }
                cache.hover_key = if user {
                    key
                } else {
                    group.map_or_else(String::new, |group| group.key.clone())
                };
                (row, owner)
            })
            .unwrap_or_else(|_| (div().into_any_element(), None))
    }
    fn eager(&self, window: &Window, cx: &App) -> bool {
        #[cfg(test)]
        if FORCE_EAGER.with(Cell::get) {
            return true;
        }
        if window.is_a11y_active()
            || gpui_component::text::has_active_text_selection(window, cx)
            || window.is_inspector_picking(cx)
        {
            return true;
        }
        let Some(view) = self.view.upgrade() else {
            return true;
        };
        let view = view.read(cx);
        let item = &self.frame.state.saved.items[self.index];
        let id = item["id"].as_str().unwrap_or("");
        let cache = self.cache.borrow();
        let html_id = item["id"]
            .as_str()
            .map_or_else(|| format!("message-{}", self.index), str::to_owned);
        let dynamic = view.html.read(cx).has_linked(&html_id)
            || self
                .frame
                .state
                .saved
                .rich_plans
                .revisions
                .iter()
                .any(|plan| plan.message_id == html_id)
            || self
                .frame
                .state
                .saved
                .rich_plans
                .errors
                .iter()
                .any(|error| error.message_id == html_id);
        dynamic
            || (self.frame.state.status.busy()
                && self.index + 1 == self.frame.state.saved.items.len())
            || self.frame.initial_transcript
            || view.preview_child.is_some()
            || view.bubble_entrances.iter().any(|pending| {
                pending
                    .optimistic
                    .as_ref()
                    .is_some_and(|bubble| bubble["id"] == item["id"])
            })
            || view.animating_bubbles.contains_key(id)
            || view
                .copied_message
                .as_ref()
                .is_some_and(|(key, _)| key == &self.owner_key || key == &cache.hover_key)
            || view.copied_code_blocks.keys().any(|key| {
                key.strip_prefix(&self.owner_key)
                    .is_some_and(|tail| tail.starts_with(":code:"))
            })
            || view.message_hover.contains(&cache.hover_key)
            || cache
                .focus
                .as_ref()
                .is_some_and(|focus| focus.contains_focused(window, cx))
            || item["content"].as_array().is_some_and(|parts| {
                parts
                    .iter()
                    .any(|part| part["url"].is_string() || part["image_url"].is_string())
            })
    }
    fn correct(&self, window: &Window) {
        let mut cache = self.cache.borrow_mut();
        cache.size = None;
        if cache.pending {
            return;
        }
        cache.pending = true;
        let cache = self.cache.clone();
        let view = self.view.clone();
        window.on_next_frame(move |_, cx| {
            cache.borrow_mut().pending = false;
            let _ = view.update(cx, |_, cx| cx.notify());
        });
    }
}
impl IntoElement for MeasuredTranscriptSlot {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for MeasuredTranscriptSlot {
    type RequestLayoutState = SlotLayout;
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
    ) -> (LayoutId, SlotLayout) {
        #[cfg(test)]
        let _phase = phases::Section::request();
        let eager = self.eager(window, cx);
        if eager {
            self.cache.borrow_mut().size = None;
        }
        let cache = self.cache.borrow();
        let owner = cache.owner.as_ref().and_then(WeakEntity::upgrade);
        let cached_size = (!eager && !cache.pending)
            .then_some(cache.size)
            .flatten()
            .filter(|_| {
                cache.source.as_ref().is_some_and(|source| {
                    std::ptr::eq::<serde_json::Value>(
                        source.as_ref(),
                        self.frame.state.saved.items[self.index].as_ref(),
                    )
                })
            })
            .filter(|_| {
                cache.group_key.as_deref()
                    == self.frame.prepared.response_prose[self.index]
                        .map(|i| self.frame.prepared.response_groups[i].key.as_str())
            })
            .filter(|_| {
                cache.text_style.as_ref() == Some(&window.text_style())
                    && cache.rem == window.rem_size()
                    && cache.viewport_width == window.viewport_size().width
            })
            .filter(|_| {
                owner.as_ref().is_some_and(|owner| {
                    let state = owner.read(cx);
                    state.layout_generation() == cache.generation
                        && state.supports_layout_projection()
                        && !state.layout_projection_focused(window, cx)
                })
            });
        drop(cache);
        if let Some(size) = cached_size {
            let mut style = gpui::Style::default();
            style.size.width = gpui::relative(1.).into();
            // Preserve the message root's own max width (user bubbles are narrower).
            style.max_size.width = rpx(
                if self.frame.state.saved.items[self.index]["type"] == "userMessage" {
                    USER_BUBBLE_MAX_W
                } else {
                    CHAT_MAX_W
                },
            )
            .into();
            let layout = window.request_measured_layout(style, move |known, available, _, _| {
                let width = known.width.unwrap_or_else(|| match available.width {
                    gpui::AvailableSpace::Definite(width) => size.width.min(width),
                    _ => size.width,
                });
                gpui::size(width, known.height.unwrap_or(size.height))
            });
            let controls = self
                .cache
                .borrow()
                .controls
                .iter()
                .map(|target| {
                    let mut element = user_control_proxy(
                        layout,
                        target,
                        self.view.clone(),
                        self.index,
                        self.frame.state.saved.items[self.index].clone(),
                    );
                    element.request_layout(window, cx);
                    element
                })
                .collect();
            (
                layout,
                SlotLayout {
                    element: None,
                    owner,
                    projection: None,
                    proxy: None,
                    controls,
                    cached_size,
                    eager,
                },
            )
        } else {
            let (mut element, owner) = self.materialize(window, cx);
            let id = element.request_layout(window, cx);
            (
                id,
                SlotLayout {
                    element: Some(element),
                    owner,
                    projection: None,
                    proxy: None,
                    controls: Vec::new(),
                    cached_size: None,
                    eager,
                },
            )
        }
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut SlotLayout,
        window: &mut Window,
        cx: &mut App,
    ) {
        #[cfg(test)]
        let _phase = phases::Section::prepaint();
        let visible = bounds.intersects(&window.content_mask().bounds);
        let width_changed = layout
            .cached_size
            .is_some_and(|size| size.width != bounds.size.width);
        if layout.element.is_none() && (visible || layout.eager || width_changed) {
            let (mut element, owner) = self.materialize(window, cx);
            element.layout_as_root(bounds.size.into(), window, cx);
            window.with_content_mask(
                width_changed.then_some(gpui::ContentMask { bounds }),
                |window| {
                    element.prepaint_at(bounds.origin, window, cx);
                },
            );
            layout.element = Some(element);
            layout.owner = owner;
            if width_changed {
                self.correct(window);
            }
        } else if let Some(element) = layout.element.as_mut() {
            element.prepaint(window, cx);
        } else {
            let cache = self.cache.borrow();
            if let Some(owner) = layout.owner.as_ref() {
                let owner_bounds = Bounds {
                    origin: bounds.origin + cache.owner_bounds.origin,
                    size: cache.owner_bounds.size,
                };
                let mut projection = gpui_component::text::TextViewProjection::new(owner.clone());
                projection.prepaint(owner_bounds, window, cx);
                layout.projection = Some(projection);
            }
            if let Some(focus) = cache.focus.as_ref() {
                let view = self.view.clone();
                let key = cache.hover_key.clone();
                let proxy_id = if self.frame.state.saved.items[self.index]["type"] == "userMessage"
                {
                    "codex-message-block"
                } else {
                    "codex-item"
                };
                let proxy = div()
                    .id((proxy_id, self.index))
                    .size_full()
                    .track_focus(focus)
                    .tab_index(0)
                    .on_hover(move |hovered, _, cx| {
                        let _ = view.update(cx, |view, cx| {
                            if *hovered {
                                view.message_hover.insert(key.clone());
                            } else {
                                view.message_hover.remove(&key);
                            }
                            cx.notify();
                        });
                    });
                let mut proxy = proxy;
                let interactivity = std::mem::take(proxy.interactivity());
                let mut proxy = FocusProxy {
                    id: (proxy_id, self.index).into(),
                    interactivity,
                    hitbox: None,
                };
                proxy.prepaint(bounds, window, cx);
                layout.proxy = Some(proxy);
            }
            for control in &mut layout.controls {
                control.prepaint(window, cx);
            }
        }
        if layout.element.is_some() && !width_changed && !layout.eager {
            if let Some(owner) = layout.owner.as_ref() {
                let state = owner.read(cx);
                if state.supports_layout_projection() {
                    let mut cache = self.cache.borrow_mut();
                    if !cache.pending {
                        cache.source = Some(self.frame.state.saved.items[self.index].clone());
                        cache.size = Some(bounds.size);
                        cache.text_style = Some(window.text_style());
                        cache.rem = window.rem_size();
                        cache.viewport_width = window.viewport_size().width;
                        cache.owner = Some(owner.downgrade());
                        cache.owner_bounds = Bounds {
                            origin: state.layout_bounds().origin - bounds.origin,
                            size: state.layout_bounds().size,
                        };
                        cache.generation = state.layout_generation();
                        cache.group_key = self.frame.prepared.response_prose[self.index]
                            .map(|i| self.frame.prepared.response_groups[i].key.clone());
                    }
                }
            }
        }
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut SlotLayout,
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        #[cfg(test)]
        let _phase = phases::Section::paint();
        let clip = layout
            .cached_size
            .filter(|size| size.width != bounds.size.width)
            .map(|_| gpui::ContentMask { bounds });
        window.with_content_mask(clip, |window| {
            if let Some(element) = layout.element.as_mut() {
                element.paint(window, cx);
            }
            if let Some(proxy) = layout.proxy.as_mut() {
                proxy.paint(bounds, window, cx);
            }
            if let Some(projection) = layout.projection.as_mut() {
                projection.paint(window, cx);
            }
            if layout.element.is_none() {
                for control in &mut layout.controls {
                    control.paint(window, cx);
                }
            }
        });
    }
}

#[cfg(test)]
thread_local! {
    pub(super) static MATERIALIZED: Cell<usize> = const { Cell::new(0) };
    pub(super) static FORCE_EAGER: Cell<bool> = const { Cell::new(false) };
    pub(super) static RENDERED_FOOTERS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

pub(super) fn force_eager() -> bool {
    #[cfg(test)]
    {
        FORCE_EAGER.with(Cell::get)
    }
    #[cfg(not(test))]
    {
        false
    }
}

/// Same-binary footer A/B control, absent from production behavior.
pub(super) fn force_footer_eager() -> bool {
    #[cfg(test)]
    {
        static EAGER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *EAGER.get_or_init(|| std::env::var("GROVE_PERF_EAGER_FOOTERS").as_deref() == Ok("1"))
    }
    #[cfg(not(test))]
    {
        false
    }
}

pub(super) fn force_header_eager() -> bool {
    #[cfg(test)]
    {
        static EAGER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *EAGER.get_or_init(|| std::env::var("GROVE_PERF_EAGER_HEADERS").as_deref() == Ok("1"))
    }
    #[cfg(not(test))]
    {
        false
    }
}

/// Response footers have a fixed chrome height. Their original Copy button
/// remains a keyboard stop when the offscreen control tree is not materialized.
pub(super) struct MeasuredResponseFooter {
    view: WeakEntity<CodexView>,
    frame: Rc<MessageFrame>,
    group: usize,
    focus: FocusHandle,
    copy_focus: FocusHandle,
}
pub(super) struct FooterLayout {
    element: Option<AnyElement>,
    proxy: Option<FocusProxy>,
    copy: AnyElement,
}
impl MeasuredResponseFooter {
    pub fn new(
        view: WeakEntity<CodexView>,
        frame: Rc<MessageFrame>,
        group: usize,
        focus: FocusHandle,
        copy_focus: FocusHandle,
    ) -> Self {
        Self {
            view,
            frame,
            group,
            focus,
            copy_focus,
        }
    }
}
impl IntoElement for MeasuredResponseFooter {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for MeasuredResponseFooter {
    type RequestLayoutState = FooterLayout;
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
    ) -> (LayoutId, FooterLayout) {
        let mut style = gpui::Style::default();
        style.size.width = gpui::relative(1.).into();
        style.size.height = rpx(CHROME_CONTROL_H).into();
        // The eager footer's child establishes this auto min-content floor.
        // An empty flex item otherwise shrinks to zero in a long scroll stack.
        style.min_size.height = rpx(CHROME_CONTROL_H).into();
        style.max_size.width = rpx(CHAT_MAX_W).into();
        style.margin.top = rpx(-SPACE_LG).into();
        let id = window.request_layout(style, [], cx);
        let group = &self.frame.prepared.response_groups[self.group];
        let view = self.view.clone();
        let key = group.key.clone();
        let prepared = self.frame.prepared.clone();
        let group_index = self.group;
        let control_id = ElementId::from(gpui::SharedString::from(format!(
            "{}:copy-proxy",
            group.key
        )));
        let mut target = div()
            .id(control_id.clone())
            .track_focus(&self.copy_focus)
            .tab_index(0)
            .role(gpui::Role::Button)
            .aria_label("Copy entire response")
            .on_key_down(move |event, _, cx| {
                if action_key(event) {
                    let _ = view.update(cx, |view, cx| {
                        view.copy_message(
                            key.clone(),
                            prepared.response_groups[group_index].copy_text.clone(),
                            cx,
                        );
                    });
                    cx.stop_propagation();
                }
            });
        let mut copy = ControlAlias {
            layout: id,
            proxy: FocusProxy {
                id: control_id,
                interactivity: std::mem::take(target.interactivity()),
                hitbox: None,
            },
        }
        .into_any_element();
        copy.request_layout(window, cx);
        (
            id,
            FooterLayout {
                element: None,
                proxy: None,
                copy,
            },
        )
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut FooterLayout,
        window: &mut Window,
        cx: &mut App,
    ) {
        let group = &self.frame.prepared.response_groups[self.group];
        let eager = window.is_a11y_active()
            || self.view.upgrade().is_some_and(|view| {
                let view = view.read(cx);
                self.focus.contains_focused(window, cx)
                    || view.message_hover.contains(&group.key)
                    || view
                        .copied_message
                        .as_ref()
                        .is_some_and(|(key, _)| key == &group.key)
                    || group.source_keys.iter().any(|key| {
                        view.message_focus
                            .get(key)
                            .is_some_and(|focus| focus.contains_focused(window, cx))
                    })
            });
        if eager || bounds.intersects(&window.content_mask().bounds) {
            let element = self
                .view
                .update(cx, |view, cx| {
                    view.response_footer(group, false, window, cx)
                })
                .ok();
            if let Some(mut element) = element {
                element.layout_as_root(bounds.size.into(), window, cx);
                element.prepaint_at(bounds.origin, window, cx);
                layout.element = Some(element);
            }
        } else {
            let view = self.view.clone();
            let key = group.key.clone();
            let id = ElementId::from(gpui::SharedString::from(group.key.clone()));
            let mut div = div()
                .id(id.clone())
                .track_focus(&self.focus)
                .tab_index(0)
                .on_hover(move |hovered, _, cx| {
                    let _ = view.update(cx, |view, cx| {
                        if *hovered {
                            view.message_hover.insert(key.clone());
                        } else {
                            view.message_hover.remove(&key);
                        }
                        cx.notify();
                    });
                });
            let mut proxy = FocusProxy {
                id,
                interactivity: std::mem::take(div.interactivity()),
                hitbox: None,
            };
            proxy.prepaint(bounds, window, cx);
            layout.proxy = Some(proxy);
            layout.copy.prepaint(window, cx);
        }
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut FooterLayout,
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(element) = layout.element.as_mut() {
            element.paint(window, cx);
        }
        if let Some(proxy) = layout.proxy.as_mut() {
            proxy.paint(bounds, window, cx);
            layout.copy.paint(window, cx);
        }
    }
}

/// Diagnostic aggregation is opt-in and absent from production. Emit the prior
/// completed frame at the next Render so deferred prepaint factories count too.
#[cfg(test)]
pub(super) mod phases {
    use std::{cell::RefCell, sync::OnceLock, time::Instant};
    #[derive(Default)]
    struct Totals {
        render_ms: f64,
        ms: [f64; 8],
        calls: [usize; 8],
        complete: bool,
    }
    thread_local! { static TOTALS: RefCell<Totals> = RefCell::default(); }
    fn enabled() -> bool {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| std::env::var("GROVE_PERF_PHASES").as_deref() == Ok("1"))
    }
    pub struct Frame(Option<Instant>);
    impl Frame {
        pub fn start() -> Self {
            if !enabled() {
                return Self(None);
            }
            TOTALS.with(|totals| {
                let old = std::mem::take(&mut *totals.borrow_mut());
                if old.complete {
                    println!(
                        "TRANSCRIPT_PHASES {}",
                        serde_json::json!({
                            "render_ms":old.render_ms, "footer_ms":old.ms[0],
                            "footer_calls":old.calls[0], "minimap_ms":old.ms[1],
                            "minimap_calls":old.calls[1],
                            "slot_request_ms":old.ms[2], "slot_request_calls":old.calls[2],
                            "slot_prepaint_ms":old.ms[3], "slot_prepaint_calls":old.calls[3],
                            "slot_paint_ms":old.ms[4], "slot_paint_calls":old.calls[4],
                            "slot_materialize_ms":old.ms[5], "slot_materialize_calls":old.calls[5],
                        "header_ms":old.ms[6], "header_calls":old.calls[6],
                        "tool_ms":old.ms[7], "tool_calls":old.calls[7],
                            "measurement":"previous_completed_render_plus_deferred_factories",
                        })
                    );
                }
            });
            Self(Some(Instant::now()))
        }
    }
    impl Drop for Frame {
        fn drop(&mut self) {
            if let Some(start) = self.0 {
                TOTALS.with(|totals| {
                    let mut totals = totals.borrow_mut();
                    totals.render_ms = start.elapsed().as_secs_f64() * 1000.;
                    totals.complete = true;
                });
            }
        }
    }
    pub struct Section(Option<Instant>, usize);
    impl Section {
        pub fn footer() -> Self {
            Self(enabled().then(Instant::now), 0)
        }
        pub fn minimap() -> Self {
            Self(enabled().then(Instant::now), 1)
        }
        pub fn request() -> Self {
            Self(enabled().then(Instant::now), 2)
        }
        pub fn prepaint() -> Self {
            Self(enabled().then(Instant::now), 3)
        }
        pub fn paint() -> Self {
            Self(enabled().then(Instant::now), 4)
        }
        pub fn materialize() -> Self {
            Self(enabled().then(Instant::now), 5)
        }
        pub fn header() -> Self {
            Self(enabled().then(Instant::now), 6)
        }
        pub fn tool() -> Self {
            Self(enabled().then(Instant::now), 7)
        }
    }
    impl Drop for Section {
        fn drop(&mut self) {
            if let Some(start) = self.0 {
                let ms = start.elapsed().as_secs_f64() * 1000.;
                TOTALS.with(|totals| {
                    let mut totals = totals.borrow_mut();
                    totals.ms[self.1] += ms;
                    totals.calls[self.1] += 1;
                });
            }
        }
    }
}

#[derive(Default)]
pub(super) struct HeaderMeasurement {
    size: Option<Size<Pixels>>,
    label: String,
    text_style: Option<TextStyle>,
    rem: Pixels,
    viewport_width: Pixels,
    pending: bool,
}

#[cfg(test)]
impl HeaderMeasurement {
    pub(super) fn measured_for_test(&self) -> bool {
        self.size.is_some()
    }
}

/// Idle collapsed work headers keep their measured border/padding and their
/// original single button focus stop. Active or expanded headers stay eager.
pub(super) struct MeasuredWorkHeader {
    view: WeakEntity<CodexView>,
    index: usize,
    key: String,
    label: String,
    focus: FocusHandle,
    cache: Rc<RefCell<HeaderMeasurement>>,
}
pub(super) struct HeaderLayout {
    element: Option<AnyElement>,
    cached_size: Option<Size<Pixels>>,
    proxy: Option<FocusProxy>,
}
impl MeasuredWorkHeader {
    pub fn new(
        view: WeakEntity<CodexView>,
        index: usize,
        key: String,
        label: String,
        focus: FocusHandle,
        cache: Rc<RefCell<HeaderMeasurement>>,
    ) -> Self {
        Self {
            view,
            index,
            key,
            label,
            focus,
            cache,
        }
    }
    fn materialize(&self, cx: &mut App) -> AnyElement {
        self.view
            .update(cx, |view, cx| {
                view.render_work_header(
                    self.index,
                    &self.key,
                    &self.label,
                    view.expanded.contains(&self.key),
                    &self.focus,
                    cx,
                )
            })
            .unwrap_or_else(|_| div().into_any_element())
    }
    fn correction(&self, window: &Window) {
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
impl IntoElement for MeasuredWorkHeader {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for MeasuredWorkHeader {
    type RequestLayoutState = HeaderLayout;
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
    ) -> (LayoutId, HeaderLayout) {
        let cache = self.cache.borrow();
        let cached_size = cache.size.filter(|_| {
            !cache.pending
                && !window.is_a11y_active()
                && !self.focus.contains_focused(window, cx)
                && !window.is_inspector_picking(cx)
                && cache.label == self.label
                && cache.text_style.as_ref() == Some(&window.text_style())
                && cache.rem == window.rem_size()
                && cache.viewport_width == window.viewport_size().width
        });
        drop(cache);
        if let Some(size) = cached_size {
            let mut style = gpui::Style::default();
            style.size.width = gpui::relative(1.).into();
            style.max_size.width = rpx(CHAT_MAX_W).into();
            style.min_size.height = size.height.into();
            let id = window.request_measured_layout(style, move |known, available, _, _| {
                let width = known.width.unwrap_or_else(|| match available.width {
                    gpui::AvailableSpace::Definite(width) => size.width.min(width),
                    _ => size.width,
                });
                gpui::size(width, known.height.unwrap_or(size.height))
            });
            (
                id,
                HeaderLayout {
                    element: None,
                    cached_size,
                    proxy: None,
                },
            )
        } else {
            let mut element = self.materialize(cx);
            let id = element.request_layout(window, cx);
            (
                id,
                HeaderLayout {
                    element: Some(element),
                    cached_size: None,
                    proxy: None,
                },
            )
        }
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut HeaderLayout,
        window: &mut Window,
        cx: &mut App,
    ) {
        let width_changed = layout
            .cached_size
            .is_some_and(|size| size.width != bounds.size.width);
        let visible = bounds.intersects(&window.content_mask().bounds);
        if layout.element.is_none()
            && (visible
                || width_changed
                || window.is_a11y_active()
                || self.focus.contains_focused(window, cx))
        {
            let mut element = self.materialize(cx);
            element.layout_as_root(bounds.size.into(), window, cx);
            window.with_content_mask(
                width_changed.then_some(gpui::ContentMask { bounds }),
                |window| {
                    element.prepaint_at(bounds.origin, window, cx);
                },
            );
            layout.element = Some(element);
            if width_changed {
                self.correction(window);
            }
        } else if let Some(element) = layout.element.as_mut() {
            element.prepaint(window, cx);
        } else {
            let id = ElementId::from(("codex-work-header", self.index));
            let view = self.view.clone();
            let key = self.key.clone();
            let mut target = div()
                .id(id.clone())
                .track_focus(&self.focus)
                .tab_index(0)
                .role(gpui::Role::Button)
                .aria_label(self.label.clone())
                .on_key_down(move |event, _, cx| {
                    if action_key(event) {
                        let _ = view.update(cx, |view, cx| view.toggle_expansion(&key, cx));
                        cx.stop_propagation();
                    }
                });
            let mut proxy = FocusProxy {
                id,
                interactivity: std::mem::take(target.interactivity()),
                hitbox: None,
            };
            proxy.prepaint(bounds, window, cx);
            layout.proxy = Some(proxy);
        }
        if layout.element.is_some() && !width_changed && !window.is_a11y_active() {
            let mut cache = self.cache.borrow_mut();
            if !cache.pending {
                cache.size = Some(bounds.size);
                cache.label.clone_from(&self.label);
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
        bounds: Bounds<Pixels>,
        layout: &mut HeaderLayout,
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let clip = layout
            .cached_size
            .filter(|size| size.width != bounds.size.width)
            .map(|_| gpui::ContentMask { bounds });
        window.with_content_mask(clip, |window| {
            if let Some(element) = layout.element.as_mut() {
                element.paint(window, cx);
            }
            if let Some(proxy) = layout.proxy.as_mut() {
                proxy.paint(bounds, window, cx);
            }
        });
    }
}

#[cfg(test)]
thread_local! { static TOOL_DETAILS: Cell<(usize, usize)> = const { Cell::new((0, 0)) }; }
#[cfg(test)]
pub(super) fn reset_tool_detail_counts() {
    TOOL_DETAILS.with(|count| count.set((0, 0)));
}
#[cfg(test)]
pub(super) fn observe_tool_detail(current_work: bool) {
    TOOL_DETAILS.with(|count| {
        let (total, current) = count.get();
        count.set((total + 1, current + usize::from(current_work)));
    });
}
#[cfg(test)]
pub(super) fn tool_detail_counts() -> (usize, usize) {
    TOOL_DETAILS.with(Cell::get)
}
