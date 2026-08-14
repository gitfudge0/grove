use gpui::{
    Anchor, AnyElement, App, Bounds, Context, Deferred, DismissEvent, Div, ElementId,
    EventEmitter, FocusHandle, Focusable, InteractiveElement as _, IntoElement, KeyBinding,
    MouseButton, ParentElement, Pixels, Point, Render, RenderOnce, Stateful, StyleRefinement,
    Styled, Subscription, Window, anchored, deferred, div, prelude::FluentBuilder as _, px,
};
use std::{cell::Cell, rc::Rc};

use crate::{
    ElementExt, Selectable, StyledExt as _, actions::Cancel, global_state::GlobalState, v_flex,
};

const CONTEXT: &str = "Popover";
pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", Cancel, Some(CONTEXT))])
}

#[derive(IntoElement)]
pub struct Popover {
    id: ElementId,
    style: StyleRefinement,
    anchor: Anchor,
    default_open: bool,
    open: Option<bool>,
    tracked_focus_handle: Option<FocusHandle>,
    trigger: Option<Box<dyn FnOnce(bool, &Window, &App) -> AnyElement + 'static>>,
    content: Option<
        Rc<
            dyn Fn(&mut PopoverState, &mut Window, &mut Context<PopoverState>) -> AnyElement
                + 'static,
        >,
    >,
    children: Vec<AnyElement>,
    /// Hotfix for the trigger element style to support w_full.
    trigger_style: Option<StyleRefinement>,
    mouse_button: MouseButton,
    appearance: bool,
    overlay_closable: bool,
    on_open_change: Option<Rc<dyn Fn(&bool, &mut Window, &mut App)>>,
}

impl Popover {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            anchor: Anchor::TopLeft,
            trigger: None,
            trigger_style: None,
            content: None,
            tracked_focus_handle: None,
            children: vec![],
            mouse_button: MouseButton::Left,
            appearance: true,
            overlay_closable: true,
            default_open: false,
            open: None,
            on_open_change: None,
        }
    }

    /// Where the popover's speech-bubble tip sits relative to the trigger; default [`Anchor::TopLeft`].
    pub fn anchor(mut self, anchor: impl Into<Anchor>) -> Self {
        self.anchor = anchor.into();
        self
    }

    pub fn mouse_button(mut self, mouse_button: MouseButton) -> Self {
        self.mouse_button = mouse_button;
        self
    }

    pub fn trigger<T>(mut self, trigger: T) -> Self
    where
        T: Selectable + IntoElement + 'static,
    {
        self.trigger = Some(Box::new(|is_open, _, _| {
            let selected = trigger.is_selected();
            trigger.selected(selected || is_open).into_any_element()
        }));
        self
    }

    /// Only initializes open state; ignored if `open` is also used.
    pub fn default_open(mut self, open: bool) -> Self {
        self.default_open = open;
        self
    }

    /// Controls the popover directly; must be used with `on_open_change` to handle state changes.
    pub fn open(mut self, open: bool) -> Self {
        self.open = Some(open);
        self
    }

    /// The `&bool` parameter is the new open state.
    pub fn on_open_change<F>(mut self, callback: F) -> Self
    where
        F: Fn(&bool, &mut Window, &mut App) + 'static,
    {
        self.on_open_change = Some(Rc::new(callback));
        self
    }

    pub fn trigger_style(mut self, style: StyleRefinement) -> Self {
        self.trigger_style = Some(style);
        self
    }

    pub fn overlay_closable(mut self, closable: bool) -> Self {
        self.overlay_closable = closable;
        self
    }

    /// Called every render; avoid creating new elements or entities in the closure.
    pub fn content<F, E>(mut self, content: F) -> Self
    where
        E: IntoElement,
        F: Fn(&mut PopoverState, &mut Window, &mut Context<PopoverState>) -> E + 'static,
    {
        self.content = Some(Rc::new(move |state, window, cx| {
            content(state, window, cx).into_any_element()
        }));
        self
    }

    /// `false` drops bg/border/shadow/padding and click-out dismissal.
    pub fn appearance(mut self, appearance: bool) -> Self {
        self.appearance = appearance;
        self
    }

    /// If unset, a new focus handle is created for the popover.
    pub fn track_focus(mut self, handle: &FocusHandle) -> Self {
        self.tracked_focus_handle = Some(handle.clone());
        self
    }

    pub(crate) fn resolved_corner(anchor: Anchor, trigger_bounds: Bounds<Pixels>) -> Point<Pixels> {
        match anchor {
            Anchor::TopLeft => trigger_bounds.origin,
            Anchor::TopCenter => trigger_bounds.top_center(),
            Anchor::TopRight => trigger_bounds.top_right(),
            Anchor::BottomLeft => Point {
                x: trigger_bounds.origin.x,
                y: trigger_bounds.origin.y - trigger_bounds.size.height,
            },
            Anchor::BottomCenter => Point {
                x: trigger_bounds.top_center().x,
                y: trigger_bounds.origin.y - trigger_bounds.size.height,
            },
            Anchor::BottomRight => Point {
                x: trigger_bounds.top_right().x,
                y: trigger_bounds.origin.y - trigger_bounds.size.height,
            },
            _ => trigger_bounds.origin,
        }
    }
}

impl ParentElement for Popover {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Styled for Popover {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

pub struct PopoverState {
    focus_handle: FocusHandle,
    pub(crate) tracked_focus_handle: Option<FocusHandle>,
    previous_focus_handle: Option<FocusHandle>,
    trigger_bounds: Bounds<Pixels>,
    trigger_bounds_captured: bool,
    open: bool,
    on_open_change: Option<Rc<dyn Fn(&bool, &mut Window, &mut App)>>,

    _dismiss_subscription: Option<Subscription>,
}

impl PopoverState {
    pub fn new(default_open: bool, cx: &mut App) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            tracked_focus_handle: None,
            previous_focus_handle: None,
            trigger_bounds: Bounds::default(),
            trigger_bounds_captured: false,
            open: default_open,
            on_open_change: None,
            _dismiss_subscription: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.toggle_open(window, cx);
        }
    }

    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            self.toggle_open(window, cx);
        }
    }

    fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.open = open;
        if self.open {
            GlobalState::global_mut(cx).register_deferred_popover(&self.focus_handle);
        } else {
            GlobalState::global_mut(cx).unregister_deferred_popover(&self.focus_handle);
        }
    }

    fn toggle_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let opening = !self.open;
        if opening {
            // Saved so it can be restored on close.
            self.previous_focus_handle = window.focused(cx);
        }
        self.set_open(opening, cx);
        if self.open {
            let state = cx.entity();
            let focus_handle = if let Some(tracked_focus_handle) = self.tracked_focus_handle.clone()
            {
                tracked_focus_handle
            } else {
                self.focus_handle.clone()
            };
            focus_handle.focus(window, cx);

            self._dismiss_subscription =
                Some(
                    window.subscribe(&cx.entity(), cx, move |_, _: &DismissEvent, window, cx| {
                        state.update(cx, |state, cx| {
                            state.dismiss(window, cx);
                        });
                        window.refresh();
                    }),
                );
        } else {
            self._dismiss_subscription = None;
            if let Some(prev) = self.previous_focus_handle.take() {
                if self.focus_handle.contains_focused(window, cx) {
                    prev.focus(window, cx);
                }
            }
        }

        if let Some(callback) = self.on_open_change.as_ref() {
            callback(&self.open, window, cx);
        }
        cx.notify();
    }

    fn on_action_cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss(window, cx);
    }
}

impl Focusable for PopoverState {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PopoverState {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl EventEmitter<DismissEvent> for PopoverState {}

impl Popover {
    pub(crate) fn render_popover<E>(
        anchor: Anchor,
        position: Rc<Cell<Point<Pixels>>>,
        content: E,
        _: &mut Window,
        _: &mut App,
    ) -> Deferred
    where
        E: IntoElement + 'static,
    {
        deferred(
            anchored()
                .snap_to_window_with_margin(px(8.))
                .anchor(anchor)
                .position(position.get())
                .child(div().relative().child(content)),
        )
        .with_priority(1)
    }

    pub(crate) fn render_popover_content(
        anchor: Anchor,
        appearance: bool,
        _: &mut Window,
        cx: &mut App,
    ) -> Stateful<Div> {
        v_flex()
            .id("content")
            .occlude()
            .tab_group()
            .when(appearance, |this| this.popover_style(cx).p_3())
            .map(|this| match anchor {
                Anchor::TopLeft | Anchor::TopCenter | Anchor::TopRight => this.top_1(),
                Anchor::BottomLeft | Anchor::BottomCenter | Anchor::BottomRight => this.bottom_1(),
                Anchor::LeftCenter | Anchor::RightCenter => this.top_1(),
            })
    }
}

impl RenderOnce for Popover {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let force_open = self.open;
        let default_open = self.default_open;
        let tracked_focus_handle = self.tracked_focus_handle.clone();
        let state = window.use_keyed_state(self.id.clone(), cx, |_, cx| {
            PopoverState::new(default_open, cx)
        });

        state.update(cx, |state, cx| {
            if let Some(tracked_focus_handle) = tracked_focus_handle {
                state.tracked_focus_handle = Some(tracked_focus_handle);
            }
            state.on_open_change = self.on_open_change.clone();
            if let Some(force_open) = force_open {
                state.set_open(force_open, cx);
            }
        });

        let open = state.read(cx).open;
        let focus_handle = state.read(cx).focus_handle.clone();
        let trigger_bounds = state.read(cx).trigger_bounds;
        let trigger_bounds_captured = state.read(cx).trigger_bounds_captured;

        let Some(trigger) = self.trigger else {
            return div().id("empty");
        };

        let parent_view_id = window.current_view();

        // Shared so the deferred Anchored element can read real trigger bounds at prepaint time.
        let position = Rc::new(Cell::new(Self::resolved_corner(
            self.anchor,
            trigger_bounds,
        )));

        let el = div()
            .id(self.id)
            .child((trigger)(open, window, cx))
            .on_mouse_down(self.mouse_button, {
                let state = state.clone();
                move |_, window, cx| {
                    cx.stop_propagation();
                    state.update(cx, |state, cx| {
                        // Force set open first, since mouse-down-out toggles it before this runs.
                        state.set_open(open, cx);
                        state.toggle_open(window, cx);
                    });
                    cx.notify(parent_view_id);
                }
            })
            .on_prepaint({
                let state = state.clone();
                let position = position.clone();
                let anchor = self.anchor;
                move |bounds, window, cx| {
                    position.set(Self::resolved_corner(anchor, bounds));
                    let first_capture = state.update(cx, |state, _| {
                        let first = !state.trigger_bounds_captured;
                        state.trigger_bounds = bounds;
                        state.trigger_bounds_captured = true;
                        first
                    });
                    // On the first bounds capture, request a new frame so the popover renders at the correct position.
                    if first_capture {
                        window.request_animation_frame();
                    }
                }
            });

        if !open || !trigger_bounds_captured {
            return el;
        }

        let popover_content =
            Self::render_popover_content(self.anchor, self.appearance, window, cx)
                .track_focus(&focus_handle)
                .key_context(CONTEXT)
                .on_action(window.listener_for(&state, PopoverState::on_action_cancel))
                .when_some(self.content, |this, content| {
                    this.child(state.update(cx, |state, cx| (content)(state, window, cx)))
                })
                .children(self.children)
                .when(self.overlay_closable, |this| {
                    this.on_mouse_down_out({
                        let state = state.clone();
                        move |_, window, cx| {
                            state.update(cx, |state, cx| {
                                state.dismiss(window, cx);
                            });
                            cx.notify(parent_view_id);
                        }
                    })
                })
                .refine_style(&self.style);

        el.child(Self::render_popover(
            self.anchor,
            position,
            popover_content,
            window,
            cx,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::MouseButton;

    #[test]
    fn test_popover_builder_chaining() {
        let popover = Popover::new("test")
            .anchor(Anchor::BottomCenter)
            .mouse_button(MouseButton::Right)
            .default_open(true)
            .appearance(false)
            .overlay_closable(false);

        assert_eq!(popover.anchor, Anchor::BottomCenter);
        assert_eq!(popover.mouse_button, MouseButton::Right);
        assert!(popover.default_open);
        assert!(!popover.appearance);
        assert!(!popover.overlay_closable);
    }

    #[test]
    fn test_resolved_corner_top_positions() {
        use gpui::px;

        let bounds = Bounds {
            origin: Point {
                x: px(100.),
                y: px(100.),
            },
            size: gpui::Size {
                width: px(200.),
                height: px(50.),
            },
        };

        let pos = Popover::resolved_corner(Anchor::TopLeft, bounds);
        assert_eq!(pos.x, px(100.));
        assert_eq!(pos.y, px(100.));

        let pos = Popover::resolved_corner(Anchor::TopCenter, bounds);
        assert_eq!(pos.x, px(200.));
        assert_eq!(pos.y, px(100.));

        let pos = Popover::resolved_corner(Anchor::TopRight, bounds);
        assert_eq!(pos.x, px(300.));
        assert_eq!(pos.y, px(100.));

        let pos = Popover::resolved_corner(Anchor::BottomLeft, bounds);
        assert_eq!(pos.x, px(100.));
        assert_eq!(pos.y, px(50.));

        let pos = Popover::resolved_corner(Anchor::BottomCenter, bounds);
        assert_eq!(pos.x, px(200.));
        assert_eq!(pos.y, px(50.));

        let pos = Popover::resolved_corner(Anchor::BottomRight, bounds);
        assert_eq!(pos.x, px(300.));
        assert_eq!(pos.y, px(50.));
    }
}
