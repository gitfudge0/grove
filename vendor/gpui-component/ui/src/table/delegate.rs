use std::ops::Range;

use gpui::{
    App, Context, Div, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    SharedString, Stateful, Styled as _, Window, div,
};

use crate::{
    ActiveTheme as _, Icon, IconName, Size, h_flex,
    menu::PopupMenu,
    table::{Column, ColumnGroup, ColumnSort, TableState, loading::Loading},
};

#[allow(unused)]
pub trait TableDelegate: Sized + 'static {
    fn columns_count(&self, cx: &App) -> usize;

    fn rows_count(&self, cx: &App) -> usize;

    /// Only called on Table prepare or refresh.
    fn column(&self, col_ix: usize, cx: &App) -> Column;

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
    }

    fn render_header(
        &mut self,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        div().id("header")
    }

    /// Default `None` means no group headers.
    fn group_headers(&self, cx: &App) -> Option<Vec<Vec<ColumnGroup>>> {
        None
    }

    fn render_group_th(
        &mut self,
        label: &SharedString,
        _col_span: usize,
        width: Pixels,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
            .w(width)
            .h_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(label.clone())
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
            .size_full()
            .child(self.column(col_ix, cx).name.clone())
    }

    /// Not including the table head row.
    fn render_tr(
        &mut self,
        row_ix: usize,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        div().id(("row", row_ix))
    }

    fn context_menu(
        &mut self,
        row_ix: usize,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        menu
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement;

    /// e.g.: `let col = self.columns.remove(col_ix); self.columns.insert(to_ix, col);`
    fn move_column(
        &mut self,
        col_ix: usize,
        to_ix: usize,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
    }

    fn render_empty(
        &mut self,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        h_flex()
            .size_full()
            .justify_center()
            .text_color(cx.theme().muted_foreground.opacity(0.6))
            .child(Icon::new(IconName::Inbox).size_12())
            .into_any_element()
    }

    fn loading(&self, cx: &App) -> bool {
        false
    }

    /// Default is a built-in Skeleton loading view.
    fn render_loading(
        &mut self,
        size: Size,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        Loading::new().size(size)
    }

    /// Default: false
    fn has_more(&self, cx: &App) -> bool {
        false
    }

    /// Rows remaining before hitting the bottom triggers `load_more`. Default: 20.
    fn load_more_threshold(&self) -> usize {
        20
    }

    /// Always called near the bottom; check for more data or lock the loading state yourself.
    fn load_more(&mut self, window: &mut Window, cx: &mut Context<TableState<Self>>) {}

    fn render_last_empty_col(
        &mut self,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        h_flex().w_3().h_full().flex_shrink_0()
    }

    /// Called frequently, so keep this fast.
    fn visible_rows_changed(
        &mut self,
        visible_range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
    }

    /// Called frequently, so keep this fast.
    fn visible_columns_changed(
        &mut self,
        visible_range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
    }

    /// For CSV export. Empty string by default; implement to support export.
    fn cell_text(&self, row_ix: usize, col_ix: usize, cx: &App) -> String {
        String::new()
    }
}
