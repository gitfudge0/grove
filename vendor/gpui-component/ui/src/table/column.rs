use std::f32;

use gpui::{
    Bounds, Context, Edges, Empty, EntityId, IntoElement, ParentElement as _, Pixels, Render,
    SharedString, Styled as _, TextAlign, Window, div, prelude::FluentBuilder, px,
};

use crate::ActiveTheme as _;

#[derive(Debug, Clone)]
pub struct Column {
    /// In most cases, matches the field name in your data source.
    pub key: SharedString,
    pub name: SharedString,
    pub align: TextAlign,
    /// `None` means not sortable.
    pub sort: Option<ColumnSort>,
    pub paddings: Option<Edges<Pixels>>,
    pub width: Pixels,
    /// A fixed column pins at the left side when scrolling horizontally.
    pub fixed: Option<ColumnFixed>,
    pub resizable: bool,
    pub movable: bool,
    /// `false` excludes this column/its cells from selection — useful for action columns (buttons, checkboxes).
    pub selectable: bool,
    pub min_width: Pixels,
    pub max_width: Pixels,
}

#[derive(Debug, Clone)]
pub struct ColumnGroup {
    pub label: SharedString,
    pub span: usize,
}

impl ColumnGroup {
    pub fn new(label: impl Into<SharedString>, span: usize) -> Self {
        Self {
            label: label.into(),
            span,
        }
    }
}

impl Default for Column {
    fn default() -> Self {
        Self {
            key: SharedString::new(""),
            name: SharedString::new(""),
            align: TextAlign::Left,
            sort: None,
            paddings: None,
            width: px(100.),
            fixed: None,
            resizable: true,
            movable: true,
            selectable: true,
            min_width: px(20.0),
            max_width: px(f32::MAX),
        }
    }
}

impl Column {
    pub fn new(key: impl Into<SharedString>, name: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            name: name.into(),
            ..Default::default()
        }
    }

    pub fn sort(mut self, sort: ColumnSort) -> Self {
        self.sort = Some(sort);
        self
    }

    /// Sortable with the default sort behavior; see [`Column::sort`] for a custom one.
    pub fn sortable(mut self) -> Self {
        self.sort = Some(ColumnSort::Default);
        self
    }

    pub fn ascending(mut self) -> Self {
        self.sort = Some(ColumnSort::Ascending);
        self
    }

    pub fn descending(mut self) -> Self {
        self.sort = Some(ColumnSort::Descending);
        self
    }

    pub fn text_center(mut self) -> Self {
        self.align = TextAlign::Center;
        self
    }

    pub fn text_right(mut self) -> Self {
        self.align = TextAlign::Right;
        self
    }

    pub fn paddings(mut self, paddings: impl Into<Edges<Pixels>>) -> Self {
        self.paddings = Some(paddings.into());
        self
    }

    pub fn p_0(mut self) -> Self {
        self.paddings = Some(Edges::all(px(0.)));
        self
    }

    pub fn width(mut self, width: impl Into<Pixels>) -> Self {
        self.width = width.into();
        self
    }

    pub fn fixed(mut self, fixed: impl Into<ColumnFixed>) -> Self {
        self.fixed = Some(fixed.into());
        self
    }

    pub fn fixed_left(mut self) -> Self {
        self.fixed = Some(ColumnFixed::Left);
        self
    }

    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    pub fn movable(mut self, movable: bool) -> Self {
        self.movable = movable;
        self
    }

    /// `false` excludes the column header and its cells from click-selection — useful for action columns.
    pub fn selectable(mut self, selectable: bool) -> Self {
        self.selectable = selectable;
        self
    }

    pub fn min_width(mut self, min_width: impl Into<Pixels>) -> Self {
        let min_width = min_width.into();
        self.min_width = min_width;
        if self.width < min_width {
            self.width = min_width;
        }
        self
    }

    pub fn max_width(mut self, max_width: impl Into<Pixels>) -> Self {
        let max_width = max_width.into();
        self.max_width = max_width;
        if self.width > max_width {
            self.width = max_width;
        }
        self
    }
}

impl FluentBuilder for Column {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnFixed {
    Left,
}

#[derive(Debug, Clone)]
pub(crate) struct ColGroup {
    pub(crate) column: Column,
    /// Runtime width, updated on resize; includes next columns' width via col_span.
    pub(crate) width: Pixels,
    pub(crate) bounds: Bounds<Pixels>,
}

impl ColGroup {
    pub(crate) fn is_resizable(&self) -> bool {
        self.column.resizable
    }
}

#[derive(Clone)]
pub(crate) struct DragColumn {
    pub(crate) entity_id: EntityId,
    pub(crate) name: SharedString,
    pub(crate) width: Pixels,
    pub(crate) col_ix: usize,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum ColumnSort {
    #[default]
    Default,
    Ascending,
    Descending,
}

impl Render for DragColumn {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_4()
            .py_1()
            .bg(cx.theme().tokens.table_head)
            .text_color(cx.theme().muted_foreground)
            .opacity(0.9)
            .border_1()
            .border_color(cx.theme().border)
            .shadow_md()
            .w(self.width)
            .min_w(px(100.))
            .max_w(px(450.))
            .child(self.name.clone())
    }
}

#[derive(Clone)]
pub(crate) struct ResizeColumn(pub (EntityId, usize));
impl Render for ResizeColumn {
    fn render(&mut self, _window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}
