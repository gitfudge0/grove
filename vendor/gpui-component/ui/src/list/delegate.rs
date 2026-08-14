use gpui::{AnyElement, App, Context, IntoElement, ParentElement as _, Styled as _, Task, Window};

use crate::{
    ActiveTheme as _, Icon, IconName, IndexPath, Selectable, h_flex,
    list::{ListState, loading::Loading},
};

/// A delegate for the List.
#[allow(unused)]
pub trait ListDelegate: Sized + 'static {
    type Item: Selectable + IntoElement;

    /// When Query Input change, this method will be called.
    /// You can perform search here.
    fn perform_search(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        Task::ready(())
    }

    /// Return the number of sections in the list, default is 1.
    /// Min value is 1.
    fn sections_count(&self, cx: &App) -> usize {
        1
    }

    /// Return the number of items in the section at the given index.
    /// NOTE: Sections with items_count == 0 skip their header and footer too.
    fn items_count(&self, section: usize, cx: &App) -> usize;

    /// Render the item at the given index. Return None to skip the item.
    /// NOTE: Every item should have same height.
    fn render_item(
        &mut self,
        ix: IndexPath,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item>;

    /// Render the section header at the given index, default is None.
    /// NOTE: Every header should have same height.
    fn render_section_header(
        &mut self,
        section: usize,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<impl IntoElement> {
        None::<AnyElement>
    }

    /// Render the section footer at the given index, default is None.
    /// NOTE: Every footer should have same height.
    fn render_section_footer(
        &mut self,
        section: usize,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<impl IntoElement> {
        None::<AnyElement>
    }

    /// Return a Element to show when list is empty.
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

    /// Returns Some(AnyElement) to render the initial state of the list, shown
    /// before the user interacts with it (e.g. last search results). Default: None.
    fn render_initial(
        &mut self,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<AnyElement> {
        None
    }

    /// Returns the loading state to show the loading view.
    fn loading(&self, cx: &App) -> bool {
        false
    }

    /// Returns a Element to show when loading, default is built-in Skeleton
    /// loading view.
    fn render_loading(
        &mut self,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        Loading
    }

    /// Set the selected index, just store the ix, don't confirm.
    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    );

    /// Set the index of the item that has been right clicked.
    fn set_right_clicked_index(
        &mut self,
        ix: Option<IndexPath>,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
    }

    /// Set the confirm and give the selected index; means the user clicked the
    /// item or pressed Enter. Always called after `set_selected_index`.
    fn confirm(&mut self, secondary: bool, window: &mut Window, cx: &mut Context<ListState<Self>>) {
    }

    /// Cancel the selection, e.g.: Pressed ESC.
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<ListState<Self>>) {}

    /// Return true to enable load more data when scrolling to the bottom.
    /// Default: false
    fn has_more(&self, cx: &App) -> bool {
        false
    }

    /// Remaining-rows threshold that triggers `load_more`; must be smaller
    /// than the total number of first-load rows. Default: 20 entities.
    fn load_more_threshold(&self) -> usize {
        20
    }

    /// Load more data when the table is scrolled to the bottom, run as a
    /// background task; check for more data or lock the loading state.
    fn load_more(&mut self, window: &mut Window, cx: &mut Context<ListState<Self>>) {}
}
