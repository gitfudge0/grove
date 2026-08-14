use std::rc::Rc;

use gpui::{
    App, Axis, ElementId, InteractiveElement as _, IntoElement, ParentElement, RenderOnce,
    StyleRefinement, Styled, Window, div, prelude::FluentBuilder as _,
};

use crate::{AxisExt, Sizable, Size, StyledExt as _, stepper::StepperItem};

#[derive(IntoElement)]
pub struct Stepper {
    id: ElementId,
    style: StyleRefinement,
    items: Vec<StepperItem>,
    step: usize,
    layout: Axis,
    disabled: bool,
    size: Size,
    text_center: bool,
    on_click: Rc<dyn Fn(&usize, &mut Window, &mut App) + 'static>,
}

impl Stepper {
    /// Default is horizontal layout with step 0 selected.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            items: Vec::new(),
            step: 0,
            layout: Axis::Horizontal,
            disabled: false,
            size: Size::default(),
            text_center: false,
            on_click: Rc::new(|_, _, _| {}),
        }
    }

    pub fn text_center(mut self, center: bool) -> Self {
        self.text_center = center;
        self
    }

    pub fn layout(mut self, layout: Axis) -> Self {
        self.layout = layout;
        self
    }

    pub fn vertical(mut self) -> Self {
        self.layout = Axis::Vertical;
        self
    }

    pub fn selected_index(mut self, index: usize) -> Self {
        self.step = index;
        self
    }

    pub fn item(mut self, item: StepperItem) -> Self {
        self.items.push(item);
        self
    }

    pub fn items(mut self, items: impl IntoIterator<Item = StepperItem>) -> Self {
        self.items.extend(items);
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// First parameter is the `step` of the clicked item.
    pub fn on_click<F>(mut self, f: F) -> Self
    where
        F: Fn(&usize, &mut Window, &mut App) + 'static,
    {
        self.on_click = Rc::new(f);
        self
    }
}

impl Sizable for Stepper {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.size = size.into();
        self
    }
}

impl Styled for Stepper {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Stepper {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let total_items = self.items.len();
        div()
            .id(self.id)
            .w_full()
            .when(self.layout.is_horizontal(), |this| this.h_flex())
            .when(self.layout.is_vertical(), |this| this.v_flex())
            .refine_style(&self.style)
            .children(self.items.into_iter().enumerate().map(|(step, item)| {
                let is_last = step + 1 == total_items;
                item.step(step)
                    .with_size(self.size)
                    .checked_step(self.step)
                    .layout(self.layout)
                    .text_center(self.text_center)
                    .when(self.disabled, |this| this.disabled(true))
                    .is_last(is_last)
                    .on_click({
                        let on_click = self.on_click.clone();
                        move |_, window, cx| {
                            on_click(&step, window, cx);
                        }
                    })
            }))
    }
}
