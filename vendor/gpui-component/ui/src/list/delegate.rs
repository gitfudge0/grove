use gpui::{AnyElement, App, Context, IntoElement, ParentElement as _, Styled as _, Task, Window};

use crate::{
    ActiveTheme as _, Icon, IconName, IndexPath, Selectable, h_flex,
    list::{ListState, loading::Loading},
};

#[allow(unused)]
pub trait ListDelegate: Sized + 'static {
    type Item: Selectable + IntoElement;

    /// Called when the query input changes.
    fn perform_search(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        Task::ready(())
    }

    /// Default and minimum is 1.
    fn sections_count(&self, cx: &App) -> usize {
        1
    }

    /// Sections with 0 items are skipped entirely, including their header and footer.
    fn items_count(&self, section: usize, cx: &App) -> usize;

    /// `None` skips the item. Every item must have the same height.
    fn render_item(
        &mut self,
        ix: IndexPath,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item>;

    /// Every header must have the same height.
    fn render_section_header(
        &mut self,
        section: usize,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<impl IntoElement> {
        None::<AnyElement>
    }

    /// Every footer must have the same height.
    fn render_section_footer(
        &mut self,
        section: usize,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<impl IntoElement> {
        None::<AnyElement>
    }

    fn render_empty(
        &mut self,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        h_flex()
            .size_full()
            .justify_center()
            .text_color(cx.theme().muted_foreground.opacity(0.6))
            .child(Icon::new(IconName::Inbox).size_12())
            .into_any_element()
    }

    /// A view shown before the user interacts, e.g. the last search results. Default `None`.
    fn render_initial(
        &mut self,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<AnyElement> {
        None
    }

    fn loading(&self, cx: &App) -> bool {
        false
    }

    /// Default is a built-in Skeleton loading view.
    fn render_loading(
        &mut self,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        Loading
    }

    /// Just stores the index; does not confirm.
    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    );

    fn set_right_clicked_index(
        &mut self,
        ix: Option<IndexPath>,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
    }

    /// Called after clicking an item or pressing Enter; always preceded by `set_selected_index`.
    fn confirm(&mut self, secondary: bool, window: &mut Window, cx: &mut Context<ListState<Self>>) {
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<ListState<Self>>) {}

    /// Default: false.
    fn has_more(&self, cx: &App) -> bool {
        false
    }

    /// Remaining rows that trigger `load_more`; must be smaller than the first-load row count. Default: 20.
    fn load_more_threshold(&self) -> usize {
        20
    }

    /// Called whenever near the bottom, so the implementation must check whether more data actually exists.
    fn load_more(&mut self, window: &mut Window, cx: &mut Context<ListState<Self>>) {}
}
