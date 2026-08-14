use gpui::{
    AnyElement, App, InteractiveElement as _, IntoElement, ParentElement, RenderOnce,
    StyleRefinement, Styled, Window, div,
};

use crate::{ActiveTheme as _, StyledExt as _};

#[derive(IntoElement)]
pub struct DialogDescription {
    style: StyleRefinement,
    children: Vec<AnyElement>,
}

impl DialogDescription {
    pub fn new() -> Self {
        Self { style: StyleRefinement::default(), children: vec![] }
    }
}

impl ParentElement for DialogDescription {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Styled for DialogDescription {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for DialogDescription {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .id("dialog-description")
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .refine_style(&self.style)
            .children(self.children)
    }
}
