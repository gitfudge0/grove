use crate::{ActiveTheme, Collapsible, h_flex, sidebar::SidebarItem, v_flex};
use gpui::{
    App, ElementId, IntoElement, ParentElement, SharedString, Styled as _, Window, div,
    prelude::FluentBuilder as _,
};

#[derive(Clone)]
pub struct SidebarGroup<E: SidebarItem + 'static> {
    label: SharedString,
    collapsed: bool,
    children: Vec<E>,
}

impl<E: SidebarItem> SidebarGroup<E> {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            collapsed: false,
            children: Vec::new(),
        }
    }

    pub fn child(mut self, child: E) -> Self {
        self.children.push(child);
        self
    }

    pub fn children(mut self, children: impl IntoIterator<Item = E>) -> Self {
        self.children.extend(children);
        self
    }
}

impl<E: SidebarItem> Collapsible for SidebarGroup<E> {
    fn is_collapsed(&self) -> bool {
        self.collapsed
    }

    fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }
}

impl<E: SidebarItem> SidebarItem for SidebarGroup<E> {
    fn render(
        self,
        id: impl Into<ElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        let id = id.into();

        v_flex()
            .relative()
            .when(!self.collapsed, |this| {
                this.child(
                    h_flex()
                        .flex_shrink_0()
                        .px_2()
                        .rounded(cx.theme().radius)
                        .text_xs()
                        .text_color(cx.theme().sidebar_foreground.opacity(0.7))
                        .h_8()
                        .child(self.label),
                )
            })
            .child(
                div()
                    .gap_2()
                    .flex_col()
                    .children(self.children.into_iter().enumerate().map(|(ix, child)| {
                        child
                            .collapsed(self.collapsed)
                            .render(format!("{}-{}", id, ix), window, cx)
                            .into_any_element()
                    })),
            )
    }
}
