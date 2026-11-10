//! Exact offscreen plan extents, preserving real keyboard and selection owners.
use super::*;
use gpui::{
    Bounds, Element, ElementId, GlobalElementId, InspectorElementId, LayoutId, Pixels, Size,
    TextStyle,
};
use gpui_component::text::{TextViewProjection, TextViewState};
use std::cell::Cell;

#[derive(Clone)]
pub(super) enum Action {
    None,
    Expand,
    Earlier,
    Next,
    Update,
    Change,
    Approve,
    Choose(String, String),
    Compose(PlanComposerTarget),
}
#[derive(Clone)]
pub(super) struct Control {
    pub focus: FocusHandle,
    pub action: Action,
    pub enabled: bool,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    order: Rc<Cell<usize>>,
    registry: std::rc::Weak<RefCell<Projection>>,
}
#[derive(Default)]
pub(super) struct Projection {
    handles: HashMap<String, FocusHandle>,
    dependency_source: std::rc::Weak<grove_core::codex::Snapshot>,
    dependencies: Rc<Dependencies>,
    pub owner: Option<WeakEntity<NativePlan>>,
    pub scroll: Option<gpui::ScrollHandle>,
    last_focus: Option<FocusHandle>,
    reveal: Option<FocusHandle>,
    pub text: HashMap<String, Entity<TextViewState>>,
    pub active_text: Vec<Entity<TextViewState>>,
    pub markdown: HashSet<EntityId>,
    pub text_targets: HashMap<EntityId, Control>,
    controls: Vec<Control>,
    next_order: usize,
    measurement: Option<Rc<Measurement>>,
    pub registry: std::rc::Weak<RefCell<Projection>>,
}
impl Projection {
    pub fn begin(&mut self) {
        self.controls.clear();
        self.active_text.clear();
        self.next_order = 0;
    }
    pub fn control(
        &mut self,
        key: String,
        action: Action,
        enabled: bool,
        cx: &mut Context<NativePlan>,
    ) -> Control {
        let focus = self
            .handles
            .entry(key.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        self.control_with_focus(key, action, enabled, focus)
    }
    pub fn control_with_focus(
        &mut self,
        _key: String,
        action: Action,
        enabled: bool,
        focus: FocusHandle,
    ) -> Control {
        let control = Control {
            focus,
            action,
            enabled,
            bounds: Rc::new(Cell::new(Bounds::default())),
            order: Rc::new(Cell::new(0)),
            registry: self.registry.clone(),
        };
        self.controls.push(control.clone());
        control
    }
}
#[derive(Default, PartialEq)]
struct Dependencies {
    parent: Option<grove_core::rich_plan::PlanDocument>,
    approval_error: Option<String>,
}
type MeasuredText = (Entity<TextViewState>, Bounds<Pixels>, usize, bool, usize);

struct Measurement {
    snapshot: Rc<Value>,
    dependencies: Rc<Dependencies>,
    flags: (bool, bool, bool, bool, bool, Option<String>),
    size: Size<Pixels>,
    text_style: TextStyle,
    rem: Pixels,
    viewport: Pixels,
    colors: (gpui::Hsla, gpui::Hsla),
    #[cfg(test)]
    controls: Vec<(Control, Bounds<Pixels>)>,
    replay: Vec<ProjectionPart>,
    text: Vec<MeasuredText>,
}
enum ProjectionPart {
    Controls(Entity<HiddenControls>),
    Text(usize),
}
struct HiddenControls {
    plan: WeakEntity<NativePlan>,
    controls: Vec<Control>,
}
impl Render for HiddenControls {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        CONTROL_RENDERS.with(|n| n.set(n.get() + 1));
        let mut body = div();
        if let Some(plan) = self.plan.upgrade() {
            for control in &self.controls {
                body = body.child(Proxy::new(control, plan.clone()));
            }
        }
        body
    }
}
fn projection_parts(
    controls: &[(Control, Bounds<Pixels>)],
    text: &[MeasuredText],
    plan: &Entity<NativePlan>,
    cx: &mut App,
) -> Vec<ProjectionPart> {
    let mut order: Vec<_> = controls
        .iter()
        .enumerate()
        .filter(|(_, (c, _))| c.enabled)
        .map(|(i, (c, _))| (c.order.get(), true, i))
        .chain(
            text.iter()
                .enumerate()
                .map(|(i, (_, _, _, _, order))| (*order, false, i)),
        )
        .collect();
    order.sort_by_key(|entry| entry.0);
    let mut result = Vec::new();
    let mut group = Vec::new();
    let flush = |group: &mut Vec<Control>, result: &mut Vec<ProjectionPart>, cx: &mut App| {
        if !group.is_empty() {
            let controls = std::mem::take(group);
            let plan = plan.downgrade();
            result.push(ProjectionPart::Controls(
                cx.new(|_| HiddenControls { plan, controls }),
            ));
        }
    };
    for (_, control, index) in order {
        if control {
            group.push(controls[index].0.clone());
        } else {
            // Dynamic text may contain keyboard targets, so retain its place
            // between the neighboring control groups in the dispatch tree.
            if !text[index].0.read(cx).supports_layout_projection() {
                flush(&mut group, &mut result, cx);
            }
            result.push(ProjectionPart::Text(index));
        }
    }
    flush(&mut group, &mut result, cx);
    result
}
fn flags(plan: &NativePlan, cx: &App) -> (bool, bool, bool, bool, bool, Option<String>) {
    (
        plan.stale(cx),
        plan.show_previous,
        plan.send_pending,
        plan.pending_approval,
        plan.composer_pending,
        plan.error.clone(),
    )
}

pub(super) struct Observed {
    element: AnyElement,
    target: Control,
}
impl Observed {
    pub fn new(element: AnyElement, target: Control) -> Self {
        Self { element, target }
    }
}
impl IntoElement for Observed {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Observed {
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
    ) -> (LayoutId, ()) {
        (self.element.request_layout(window, cx), ())
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
        self.target.bounds.set(bounds);
        if let Some(registry) = self.target.registry.upgrade() {
            let mut p = registry.borrow_mut();
            self.target.order.set(p.next_order);
            p.next_order += 1;
            if p.reveal.as_ref() == Some(&self.target.focus) {
                p.reveal = None;
                if let (Some(scroll), Some(owner)) = (p.scroll.clone(), p.owner.clone()) {
                    let viewport = scroll.bounds();
                    let delta = if bounds.top() < viewport.top() {
                        viewport.top() - bounds.top()
                    } else if bounds.bottom() > viewport.bottom() {
                        viewport.bottom() - bounds.bottom()
                    } else {
                        gpui::px(0.)
                    };
                    if delta != gpui::px(0.) {
                        window.on_next_frame(move |_, cx| {
                            let mut offset = scroll.offset();
                            offset.y += delta;
                            scroll.set_offset(offset);
                            let _ = owner.update(cx, |_, cx| cx.notify());
                        });
                    }
                }
            }
        }
        self.element.prepaint(window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        (): &mut (),
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.element.paint(window, cx);
    }
}
struct Proxy {
    focus: FocusHandle,
    interaction: gpui::Interactivity,
    bounds: Bounds<Pixels>,
}
impl Proxy {
    fn new(target: &Control, plan: Entity<NativePlan>) -> Self {
        let action = target.action.clone();
        let mut element =
            div()
                .track_focus(&target.focus)
                .tab_index(0)
                .on_key_down(move |event, window, cx| {
                    if !matches!(action, Action::None)
                        && matches!(event.keystroke.key.as_str(), "enter" | "space")
                    {
                        plan.update(cx, |plan, cx| plan.activate(&action, window, cx));
                        cx.stop_propagation();
                    }
                });
        Self {
            focus: target.focus.clone(),
            interaction: std::mem::take(element.interactivity()),
            bounds: Bounds::default(),
        }
    }
    fn prepaint_proxy(&mut self, window: &mut Window, cx: &mut App) {
        // Hidden controls have no pointer surface or retained hover state. Their
        // real focus handles still occupy distinct dispatch nodes, and paint
        // registers them in the normal tab order. Focus reifies the real card.
        window.set_focus_handle(&self.focus, cx);
    }
    fn paint_proxy(&mut self, window: &mut Window, cx: &mut App) {
        self.interaction
            .paint(None, None, self.bounds, None, window, cx, |_, _, _| {});
    }
}
impl IntoElement for Proxy {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Proxy {
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
    ) -> (LayoutId, ()) {
        (window.request_layout(gpui::Style::default(), [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.prepaint_proxy(window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        (): &mut (),
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.paint_proxy(window, cx);
    }
}
enum Replay {
    Control(AnyElement),
    Text(TextViewProjection),
    Live(LiveText),
}
struct LiveText {
    element: AnyElement,
    namespace: EntityId,
    bounds: Bounds<Pixels>,
}
impl LiveText {
    fn new(owner: &Entity<TextViewState>, namespace: EntityId, window: &Window) -> Self {
        let element = div()
            .id("native-plan-card")
            .w_full()
            .text_size(rpx(TEXT_BODY))
            .text_color(c::FG())
            .child(
                div().id("native-plan-body").min_w_0().child(
                    TextView::new(owner)
                        .style(super::super::codex_view::markdown_style(window))
                        .markdown_extensions(super::super::mermaid::extensions())
                        .selectable(true),
                ),
            )
            .into_any_element();
        Self {
            element,
            namespace,
            bounds: Bounds::default(),
        }
    }
    fn prepaint(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Size<Pixels> {
        self.bounds = bounds;
        window.with_id(ElementId::View(self.namespace), |window| {
            let size = self.element.layout_as_root(
                gpui::size(
                    gpui::AvailableSpace::Definite(bounds.size.width),
                    gpui::AvailableSpace::MaxContent,
                ),
                window,
                cx,
            );
            window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                self.element.prepaint_at(bounds.origin, window, cx);
            });
            size
        })
    }
    fn paint(&mut self, window: &mut Window, cx: &mut App) {
        window.with_id(ElementId::View(self.namespace), |window| {
            window.with_content_mask(
                Some(gpui::ContentMask {
                    bounds: self.bounds,
                }),
                |window| self.element.paint(window, cx),
            );
        });
    }
}
pub(super) struct Plan {
    owner: Entity<NativePlan>,
}
impl Plan {
    pub fn new(owner: Entity<NativePlan>) -> Self {
        Self { owner }
    }
}
impl IntoElement for Plan {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
pub(super) struct Layout {
    element: Option<AnyElement>,
    cached: Option<Rc<Measurement>>,
    replay: Vec<Replay>,

    key: Rc<Value>,
    flags: (bool, bool, bool, bool, bool, Option<String>),
    eager: bool,
}
#[cfg(test)]
thread_local! {pub(super) static MATERIALIZED:Cell<usize>=const {Cell::new(0)}; pub(super) static CONTROL_RENDERS:Cell<usize>=const {Cell::new(0)}; static TEST_EAGER:Cell<bool>=const{Cell::new(false)};}
fn eager_control_registration() -> bool {
    #[cfg(test)]
    {
        std::env::var_os("GROVE_PERF_EAGER_CONTROL_REGISTRATION").is_some()
    }
    #[cfg(not(test))]
    {
        false
    }
}
fn force_eager() -> bool {
    #[cfg(test)]
    {
        TEST_EAGER.with(Cell::get) || std::env::var("GROVE_PERF_EAGER_PLANS").as_deref() == Ok("1")
    }
    #[cfg(not(test))]
    {
        false
    }
}
impl Element for Plan {
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
        let plan = self.owner.read(cx);
        let key = plan.snapshot(cx);
        let flags = flags(plan, cx);
        let mut projection = plan.projection.borrow_mut();
        let source = plan.session.read(cx).state.shared();
        let weak = Rc::downgrade(&source);
        if !projection.dependency_source.ptr_eq(&weak) {
            let store = &source.saved.rich_plans;
            let parent = key["document"]["finalized_from"]["revision_id"]
                .as_str()
                .and_then(|revision| store.revision(&plan.plan_id, revision))
                .map(|revision| revision.document.clone());
            let dependencies = Dependencies {
                parent,
                approval_error: store.approval_error(&plan.plan_id, &plan.revision_id),
            };
            if projection.dependencies.as_ref() != &dependencies {
                projection.dependencies = Rc::new(dependencies);
            }
            projection.dependency_source = weak;
        }
        let focused = window.focused(cx);
        if projection.last_focus != focused {
            projection.reveal.clone_from(&focused);
            projection.last_focus = focused;
        }
        let eager = force_eager()
            || plan.expanded(cx)
            || cx
                .try_global::<PlanUi>()
                .and_then(|ui| ui.peek.as_ref())
                .is_some_and(|p| p.entity_id() == self.owner.entity_id())
            || key["document"]["markdown"].is_null()
            || window.is_a11y_active()
            || window.is_inspector_picking(cx)
            || gpui_component::text::has_active_text_selection(window, cx)
            || projection
                .controls
                .iter()
                .any(|control| control.focus.contains_focused(window, cx))
            || projection.active_text.iter().any(|owner| {
                owner.read(cx).layout_projection_focused(window, cx)
                    || owner.read(cx).has_view_selection()
            });
        let cached = projection
            .measurement
            .as_ref()
            .filter(|m| {
                !eager
                    && Rc::ptr_eq(&m.snapshot, &key)
                    && Rc::ptr_eq(&m.dependencies, &projection.dependencies)
                    && m.flags == flags
                    && m.rem == window.rem_size()
                    && m.viewport == window.viewport_size().width
                    && m.text_style == window.text_style()
                    && m.colors == (c::FG(), c::BG())
                    && m.text.iter().all(|(owner, _, generation, markdown, _)| {
                        owner.read(cx).layout_generation() == *generation
                            && (*markdown || owner.read(cx).supports_layout_projection())
                    })
            })
            .cloned();
        drop(projection);
        let (id, element) = if let Some(m) = &cached {
            let mut style = gpui::Style::default();
            style.size.width = gpui::relative(1.).into();
            let size = m.size;
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
            let mut element = self.owner.clone().into_any_element();
            let id = element.request_layout(window, cx);
            (id, Some(element))
        };
        (
            id,
            Layout {
                element,
                cached,
                replay: Vec::new(),

                key,
                flags,
                eager,
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
            let mut element = self.owner.clone().into_any_element();
            element.layout_as_root(bounds.size.into(), window, cx);
            element.prepaint_at(bounds.origin, window, cx);
            layout.element = Some(element);
            if width_changed {
                self.owner.read(cx).projection.borrow_mut().measurement = None;
                let owner = self.owner.downgrade();
                window.on_next_frame(move |_, cx| {
                    let _ = owner.update(cx, |_, cx| cx.notify());
                });
            }
        } else if let Some(element) = &mut layout.element {
            element.prepaint(window, cx);
        }
        if layout.element.is_some() && !width_changed && !layout.eager {
            let projection = self.owner.read(cx).projection.clone();
            let mut projection = projection.borrow_mut();
            if projection.active_text.iter().all(|owner| {
                projection.markdown.contains(&owner.entity_id())
                    || owner.read(cx).supports_layout_projection()
            }) {
                let mut controls: Vec<_> = projection
                    .controls
                    .iter()
                    .cloned()
                    .map(|control| {
                        let mut relative = control.bounds.get();
                        relative.origin -= bounds.origin;
                        (control, relative)
                    })
                    .collect();
                controls.sort_by_key(|(c, _)| c.order.get());
                let text: Vec<_> = projection
                    .active_text
                    .iter()
                    .map(|owner| {
                        let state = owner.read(cx);
                        let mut relative = state.layout_bounds();
                        relative.origin -= bounds.origin;
                        (
                            owner.clone(),
                            relative,
                            state.layout_generation(),
                            projection.markdown.contains(&owner.entity_id()),
                            projection.text_targets[&owner.entity_id()].order.get(),
                        )
                    })
                    .collect();
                let replay = projection_parts(&controls, &text, &self.owner, cx);
                projection.measurement = Some(Rc::new(Measurement {
                    snapshot: layout.key.clone(),
                    dependencies: projection.dependencies.clone(),
                    flags: layout.flags.clone(),
                    size: bounds.size,
                    text_style: window.text_style(),
                    rem: window.rem_size(),
                    viewport: window.viewport_size().width,
                    colors: (c::FG(), c::BG()),
                    #[cfg(test)]
                    controls,
                    replay,
                    text,
                }));
            }
        } else if layout.element.is_none() {
            if let Some(m) = &layout.cached {
                for part in &m.replay {
                    if let ProjectionPart::Controls(group) = part {
                        let mut style = gpui::StyleRefinement::default();
                        style.size.width = Some(gpui::px(0.).into());
                        style.size.height = Some(gpui::px(0.).into());
                        let mut proxy = if eager_control_registration() {
                            group.clone().into_any_element()
                        } else {
                            group.clone().cached(style).into_any_element()
                        };
                        proxy.layout_as_root(
                            gpui::size(gpui::px(0.), gpui::px(0.)).into(),
                            window,
                            cx,
                        );
                        // Keyboard-only nodes have no pointer geometry. A stable
                        // origin lets GPUI retain dispatch and tab registration
                        // while the actual transcript text continues to scroll.
                        proxy.prepaint_at(gpui::Point::default(), window, cx);
                        layout.replay.push(Replay::Control(proxy));
                    } else if let ProjectionPart::Text(index) = part {
                        let (owner, relative, _, markdown, _) = &m.text[*index];
                        let mut b = *relative;
                        b.origin += bounds.origin;
                        if owner.read(cx).supports_layout_projection() {
                            let mut text = TextViewProjection::new(owner.clone());
                            text.prepaint(b, window, cx);
                            layout.replay.push(Replay::Text(text));
                        } else {
                            debug_assert!(*markdown);
                            let mut live = LiveText::new(owner, self.owner.entity_id(), window);
                            let size = live.prepaint(b, window, cx);
                            if (size.height - b.size.height).abs() > gpui::px(0.5)
                                || size.width != b.size.width
                            {
                                self.owner.read(cx).projection.borrow_mut().measurement = None;
                                let owner = self.owner.downgrade();
                                window.on_next_frame(move |_, cx| {
                                    let _ = owner.update(cx, |_, cx| cx.notify());
                                });
                            }
                            layout.replay.push(Replay::Live(live));
                        }
                    }
                }
            }
        }
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        layout: &mut Layout,
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(element) = &mut layout.element {
            element.paint(window, cx);
        }
        for replay in &mut layout.replay {
            match replay {
                Replay::Control(proxy) => proxy.paint(window, cx),
                Replay::Text(text) => text.paint(window, cx),
                Replay::Live(text) => text.paint(window, cx),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grove_core::codex::{Connection, Handle, Provider, SavedSession, Snapshot, Status};
    use std::sync::mpsc;
    struct History {
        plan: Entity<NativePlan>,
        scroll: gpui::ScrollHandle,
        width: Pixels,
        after: FocusHandle,
    }
    impl Render for History {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("measured-plan-history")
                .on_key_down(crate::views::shell::traverse_unhandled_tab)
                .w(self.width)
                .h(gpui::px(240.))
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .child(Plan::new(self.plan.clone()))
                .child(div().h(gpui::px(1600.)))
                .child(
                    div()
                        .id("after-plan")
                        .track_focus(&self.after)
                        .tab_index(0)
                        .h(gpui::px(24.))
                        .child("After plan"),
                )
        }
    }
    fn draw(view: &Entity<History>, cx: &mut gpui::VisualTestContext) {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(16));
        cx.update(gpui::Window::simulate_next_frame);
        cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
    }
    fn exercise(cx: &mut gpui::TestAppContext, provider: Provider, diagram: bool) {
        cx.update(|cx| {
            gpui_component::init(cx);
            c::sync_component_theme(cx);
            cx.set_reduce_motion(true);
        });
        let (handle, _actions) = Handle::channel();
        let (_updates, receive) = mpsc::channel();
        let mut saved:SavedSession=serde_json::from_value(json!({"key":"measured-plan","provider":provider,"project":"Test","cwd":"/tmp","label":"Test","roots":[],"thread_id":"measured-plan","items":[],"draft":"", "rich_plans":{"revisions":[{"message_id":"answer","source":"plan.grove-plan.json","assets":[],"document":{"schema_version":1,"plan_id":"plan","revision_id":"r1","title":"Measured","markdown":"# Measured plan\n\n## Access\nChoose access while preserving full plan text.","decisions":[{"id":"access","section":"Access","prompt":"Who can invite?","options":[{"id":"admin","label":"Admins","description":"Owners can invite."},{"id":"member","label":"Members","description":"Members can invite."}]}]}}]}})).unwrap();
        if diagram {
            saved.rich_plans.revisions[0]
                .document
                .markdown
                .as_mut()
                .unwrap()
                .push_str("\n\n## Flow\n```mermaid\nflowchart LR\nA-->B\n```\n");
        }
        let mut snapshot = Snapshot::new(saved);
        snapshot.status = Status::Idle;
        let mut history = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let session = cx.new(|cx| {
                CodexSession::from_connection(
                    snapshot,
                    Connection {
                        handle,
                        updates: receive.into(),
                    },
                    cx,
                )
            });
            let html = cx.new(|_| {
                super::super::super::inline_html::HtmlSession::new(
                    std::path::PathBuf::from("/tmp"),
                    gpui::ScrollHandle::new(),
                )
            });
            let plan = cx.new(|cx| NativePlan::new(session, html, "plan".into(), "r1".into(), cx));
            let scroll = gpui::ScrollHandle::new();
            plan.update(cx, |plan, _| plan.set_transcript_scroll(scroll.clone()));
            let view = cx.new(|cx| History {
                plan,
                scroll,
                width: gpui::px(600.),
                after: cx.focus_handle().tab_stop(true),
            });
            history = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        let history = history.unwrap();
        cx.simulate_resize(gpui::size(gpui::px(640.), gpui::px(300.)));
        for _ in 0..3 {
            draw(&history, cx);
        }
        let (plan, height) = history.read_with(cx, |view, cx| {
            let plan = view.plan.clone();
            let height = plan
                .read(cx)
                .projection
                .borrow()
                .measurement
                .as_ref()
                .expect("plan must measure stable text")
                .size
                .height;
            (plan, height)
        });
        cx.update(|_, cx| {
            history.update(cx, |view, cx| {
                view.scroll
                    .set_offset(gpui::point(gpui::px(0.), -height - gpui::px(100.)));
                cx.notify();
            });
        });
        draw(&history, cx);
        MATERIALIZED.with(|n| n.set(0));
        draw(&history, cx);
        assert_eq!(
            MATERIALIZED.with(Cell::get),
            0,
            "offscreen complete plan must not render its controls"
        );
        plan.read_with(cx, |plan, cx| {
            let projection = plan.projection.borrow();
            assert_eq!(projection.measurement.as_ref().unwrap().size.height, height);
            assert!(projection
                .active_text
                .iter()
                .any(|text| text.read(cx).source().contains("preserving full plan text")));
        });
        // Same-binary eager geometry and metadata-only snapshot updates.
        TEST_EAGER.with(|flag| flag.set(true));
        draw(&history, cx);
        assert!(
            (cx.debug_bounds("native-plan-card").unwrap().size.height - height).abs()
                < gpui::px(0.5)
        );
        TEST_EAGER.with(|flag| flag.set(false));
        draw(&history, cx);
        MATERIALIZED.with(|n| n.set(0));
        cx.update(|_, cx| {
            plan.read(cx).session.clone().update(cx, |session, cx| {
                session.state.model = Some("metadata-only".into());
                cx.notify();
            });
        });
        draw(&history, cx);
        assert_eq!(
            MATERIALIZED.with(Cell::get),
            0,
            "metadata must preserve offscreen measurements"
        );
        // Resize the pane at a fixed window size; cached extents must reflow.
        cx.update(|_, cx| {
            history.update(cx, |view, cx| {
                view.width = gpui::px(380.);
                cx.notify();
            });
        });
        for _ in 0..3 {
            draw(&history, cx);
        }
        plan.read_with(cx, |plan, _| {
            assert!(
                plan.projection
                    .borrow()
                    .measurement
                    .as_ref()
                    .unwrap()
                    .size
                    .width
                    <= gpui::px(380.)
            );
        });
        cx.update(|_, cx| {
            history.update(cx, |view, cx| {
                view.width = gpui::px(600.);
                cx.notify();
            });
        });
        for _ in 0..3 {
            draw(&history, cx);
        }
        // Selection re-materializes the complete document, including offscreen text.
        let text = plan.read_with(cx, |plan, _| {
            plan.projection.borrow().active_text[0].clone()
        });
        cx.update(|_, cx| text.update(cx, gpui_component::text::TextViewState::select_all));
        MATERIALIZED.with(|n| n.set(0));
        draw(&history, cx);
        assert!(MATERIALIZED.with(Cell::get) > 0);
        text.read_with(cx, |text, _| assert!(!text.selected_text().is_empty()));
        cx.update(|_, cx| text.update(cx, gpui_component::text::TextViewState::clear_selection));
        draw(&history, cx);
        // Cross from an external sentinel into the last projected plan control.
        let last = plan.read_with(cx, |plan, _| {
            let p = plan.projection.borrow();
            p.measurement
                .as_ref()
                .unwrap()
                .controls
                .iter().rfind(|(control, _)| control.enabled)
                .unwrap()
                .0
                .focus
                .clone()
        });
        cx.update(|window, cx| {
            let focus = history.read(cx).after.clone();
            focus.focus(window, cx);
        });
        draw(&history, cx);
        MATERIALIZED.with(|n| n.set(0));
        draw(&history, cx);
        assert_eq!(MATERIALIZED.with(Cell::get), 0);
        cx.simulate_keystrokes("shift-tab");
        cx.update(|window, _| {
            assert!(
                last.is_focused(window),
                "reverse Tab must reach the projected last control"
            );
        });
        for _ in 0..3 {
            draw(&history, cx);
        }
        let target_bounds = plan.read_with(cx, |plan, _| {
            plan.projection
                .borrow()
                .controls
                .iter()
                .find(|control| control.focus == last)
                .unwrap()
                .bounds
                .get()
        });
        let viewport = history.read_with(cx, |view, _| view.scroll.bounds());
        assert!(
            target_bounds.top() >= viewport.top() && target_bounds.bottom() <= viewport.bottom(),
            "projected control must reveal after Tab"
        );
        cx.simulate_keystrokes("tab");
        cx.update(|window, cx| {
            assert!(
                history.read(cx).after.is_focused(window),
                "forward Tab must leave plan through external sentinel"
            );
        });
        // Restore the exact hidden control handle through the projected dispatch tree.
        cx.update(|window, cx| {
            plan.update(cx, |plan, cx| {
                plan.projection.borrow().handles["option:access:admin"].focus(window, cx);
            });
        });
        draw(&history, cx);
        draw(&history, cx);
        // Walk real Tab order between adjacent options, then backwards.
        cx.simulate_keystrokes("tab");
        draw(&history, cx);
        cx.update(|window, cx| {
            plan.read_with(cx, |plan, _| {
                assert!(plan.projection.borrow().handles["option:access:member"].is_focused(window));
            });
        });
        cx.simulate_keystrokes("shift-tab");
        draw(&history, cx);
        cx.update(|window, cx| {
            plan.read_with(cx, |plan, _| {
                assert!(plan.projection.borrow().handles["option:access:admin"].is_focused(window));
            });
        });
        let button = cx
            .debug_bounds("native-plan-option-admin")
            .expect("focused option must materialize");
        let viewport = history.read_with(cx, |view, _| view.scroll.bounds());
        assert!(
            button.top() >= viewport.top() && button.bottom() <= viewport.bottom(),
            "keyboard target must scroll into view"
        );
        cx.simulate_keystrokes("space");
        cx.run_until_parked();
        plan.read_with(cx, |plan, cx| {
            assert_eq!(plan.snapshot(cx)["review"]["answers"]["access"], "admin");
        });
        // A local answer changes projection identity even without session acknowledgment.
        let before = plan.read_with(cx, super::super::NativePlan::snapshot);
        cx.update(|_, cx| {
            plan.update(cx, |plan, cx| {
                plan.choose("access", "member", cx);
            });
        });
        draw(&history, cx);
        plan.read_with(cx, |plan, cx| {
            let after = plan.snapshot(cx);
            assert!(!Rc::ptr_eq(&before, &after));
            assert_eq!(after["review"]["answers"]["access"], "member");
        });
        cx.update(|window, cx| {
            window.blur();
            grove_core::theme::set_by_name("grove-light");
            c::sync_component_theme(cx);
            gpui_component::Theme::global_mut(cx).font_size = gpui::px(20.);
        });
        for _ in 0..3 {
            draw(&history, cx);
        }
        plan.read_with(cx, |plan, _| {
            let p = plan.projection.borrow();
            let m = p.measurement.as_ref().unwrap();
            assert_eq!(m.rem, gpui::px(20.));
            assert_eq!(m.colors, (c::FG(), c::BG()));
        });
        // Finalized summary and approval status depend on the parent revision too.
        let session = plan.read_with(cx, |plan, _| plan.session.clone());
        cx.update(|_, cx| {
            session.update(cx, |session, cx| {
                let store = &mut session.state.saved.rich_plans;
                let mut previous = store.revisions[0].clone();
                previous.document.revision_id = "r0".into();
                store.revisions[0].document.finalized_from =
                    Some(grove_core::rich_plan::PlanFinalization {
                        revision_id: "r0".into(),
                        answers: HashMap::from([("access".into(), "Admins".into())]),
                    });
                store.revisions.insert(0, previous);
                cx.notify();
            });
        });
        cx.update(|window, cx| {
            window.blur();
            history.update(cx, |view, cx| {
                view.scroll.scroll_to_bottom();
                cx.notify();
            });
        });
        for _ in 0..3 {
            draw(&history, cx);
        }
        let own = plan.read_with(cx, super::super::NativePlan::snapshot);
        MATERIALIZED.with(|n| n.set(0));
        cx.update(|_, cx| {
            session.update(cx, |session, cx| {
                session.state.saved.rich_plans.revisions[0]
                    .document
                    .decisions[0]
                    .prompt = "Changed parent prompt".into();
                cx.notify();
            });
        });
        draw(&history, cx);
        assert!(
            MATERIALIZED.with(Cell::get) > 0,
            "parent dependency must invalidate extent"
        );
        plan.read_with(cx, |plan, cx| {
            assert!(Rc::ptr_eq(&own, &plan.snapshot(cx)));
            assert_eq!(
                plan.projection
                    .borrow()
                    .dependencies
                    .parent
                    .as_ref()
                    .unwrap()
                    .decisions[0]
                    .prompt,
                "Changed parent prompt"
            );
        });
    }
    #[gpui::test]
    fn codex_offscreen_plan_preserves_controls(cx: &mut gpui::TestAppContext) {
        exercise(cx, Provider::Codex, false);
    }
    #[gpui::test]
    fn claude_offscreen_plan_preserves_controls(cx: &mut gpui::TestAppContext) {
        exercise(cx, Provider::Claude, false);
    }
    #[gpui::test]
    fn codex_offscreen_plan_keeps_dynamic_diagram_live(cx: &mut gpui::TestAppContext) {
        exercise(cx, Provider::Codex, true);
    }
    #[gpui::test]
    fn claude_offscreen_plan_keeps_dynamic_diagram_live(cx: &mut gpui::TestAppContext) {
        exercise(cx, Provider::Claude, true);
    }
}
