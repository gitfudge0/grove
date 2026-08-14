use crate::{
    ActiveTheme, Sizable, Size,
    actions::{
        Cancel, SelectDown, SelectFirst, SelectLast, SelectNextColumn, SelectPageDown,
        SelectPageUp, SelectPrevColumn, SelectUp,
    },
    table::{TableDelegate, TableState},
};
use gpui::{
    App, Edges, Entity, Focusable, InteractiveElement, IntoElement, KeyBinding, ParentElement,
    RenderOnce, Styled, Window, div, prelude::FluentBuilder,
};

const CONTEXT: &'static str = "DataTable";
pub(super) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", Cancel, Some(CONTEXT)),
        KeyBinding::new("up", SelectUp, Some(CONTEXT)),
        KeyBinding::new("down", SelectDown, Some(CONTEXT)),
        KeyBinding::new("left", SelectPrevColumn, Some(CONTEXT)),
        KeyBinding::new("right", SelectNextColumn, Some(CONTEXT)),
        KeyBinding::new("home", SelectFirst, Some(CONTEXT)),
        KeyBinding::new("end", SelectLast, Some(CONTEXT)),
        KeyBinding::new("pageup", SelectPageUp, Some(CONTEXT)),
        KeyBinding::new("pagedown", SelectPageDown, Some(CONTEXT)),
        KeyBinding::new("tab", SelectNextColumn, Some(CONTEXT)),
        KeyBinding::new("shift-tab", SelectPrevColumn, Some(CONTEXT)),
    ]);
}

pub(super) struct TableOptions {
    pub(super) scrollbar_visible: Edges<bool>,
    pub(super) stripe: bool,
    pub(super) bordered: bool,
    pub(super) size: Size,
}

impl Default for TableOptions {
    fn default() -> Self {
        Self {
            scrollbar_visible: Edges::all(true),
            stripe: false,
            bordered: true,
            size: Size::default(),
        }
    }
}

/// Row/column/cell selection, virtual scrolling, resizable/movable/fixed/sortable columns, context menus.
/// Cell selection mode (via [`TableState::cell_selectable()`]) adds a row-header selection column (hide with [`TableState::row_header()`]) and cell-level keyboard nav.
///
/// ```rust,ignore
/// let table_state = cx.new(|cx| {
///     TableState::new(delegate, cx)
///         .cell_selectable(true)
///         .row_selectable(true)
/// });
///
/// DataTable::new(&table_state)
///     .stripe(true)
///     .bordered(true)
/// ```
#[derive(IntoElement)]
pub struct DataTable<D: TableDelegate> {
    state: Entity<TableState<D>>,
    options: TableOptions,
}

impl<D> DataTable<D>
where
    D: TableDelegate,
{
    pub fn new(state: &Entity<TableState<D>>) -> Self {
        Self {
            state: state.clone(),
            options: TableOptions::default(),
        }
    }

    /// Default false.
    pub fn stripe(mut self, stripe: bool) -> Self {
        self.options.stripe = stripe;
        self
    }

    /// Default true.
    pub fn bordered(mut self, bordered: bool) -> Self {
        self.options.bordered = bordered;
        self
    }

    pub fn scrollbar_visible(mut self, vertical: bool, horizontal: bool) -> Self {
        self.options.scrollbar_visible = Edges {
            right: vertical,
            bottom: horizontal,
            ..Default::default()
        };
        self
    }
}

impl<D> Sizable for DataTable<D>
where
    D: TableDelegate,
{
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.options.size = size.into();
        self
    }
}

impl<D> RenderOnce for DataTable<D>
where
    D: TableDelegate,
{
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let bordered = self.options.bordered;
        let focus_handle = self.state.focus_handle(cx);
        self.state.update(cx, |state, _| {
            state.options = self.options;
        });

        div()
            .id("table")
            .size_full()
            .key_context(CONTEXT)
            .track_focus(&focus_handle)
            .on_action(window.listener_for(&self.state, TableState::action_cancel))
            .on_action(window.listener_for(&self.state, TableState::action_select_next))
            .on_action(window.listener_for(&self.state, TableState::action_select_prev))
            .on_action(window.listener_for(&self.state, TableState::action_select_next_col))
            .on_action(window.listener_for(&self.state, TableState::action_select_prev_col))
            .on_action(window.listener_for(&self.state, TableState::action_select_first_column))
            .on_action(window.listener_for(&self.state, TableState::action_select_last_column))
            .on_action(window.listener_for(&self.state, TableState::action_select_page_up))
            .on_action(window.listener_for(&self.state, TableState::action_select_page_down))
            .bg(cx.theme().tokens.table)
            .when(bordered, |this| {
                this.rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
            })
            .child(self.state)
    }
}
