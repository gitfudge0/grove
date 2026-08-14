use gpui::{
    AnyElement, App, Div, Hsla, IntoElement, ParentElement, Pixels, Point, RenderOnce,
    SharedString, Size, StyleRefinement, Styled, Window, deferred, div, prelude::FluentBuilder, px,
};

use crate::{ActiveTheme, Colorize, StyledExt, h_flex, v_flex};

#[derive(Default)]
pub enum CrossLineAxis {
    #[default]
    Vertical,
    Horizontal,
    Both,
}

impl CrossLineAxis {
    #[inline]
    pub fn show_vertical(&self) -> bool {
        matches!(self, CrossLineAxis::Vertical | CrossLineAxis::Both)
    }

    #[inline]
    pub fn show_horizontal(&self) -> bool {
        matches!(self, CrossLineAxis::Horizontal | CrossLineAxis::Both)
    }
}

#[derive(IntoElement)]
pub struct CrossLine {
    point: Point<Pixels>,
    /// `(start, length)`; `length` of `None` spans the full height.
    vertical: (f32, Option<f32>),
    /// `(start, length)`; `length` of `None` spans the full width.
    horizontal: (f32, Option<f32>),
    thickness: Pixels,
    /// `true` (default) draws a dashed hairline; `false` a solid band.
    dashed: bool,
    direction: CrossLineAxis,
}

impl CrossLine {
    pub fn new(point: Point<Pixels>) -> Self {
        Self {
            point,
            vertical: (0., None),
            horizontal: (0., None),
            thickness: px(1.),
            dashed: true,
            direction: Default::default(),
        }
    }

    /// Solid translucent band instead of the default dashed hairline.
    pub fn band(mut self, thickness: impl Into<Pixels>) -> Self {
        self.thickness = thickness.into();
        self.dashed = false;
        self
    }

    pub fn horizontal(mut self) -> Self {
        self.direction = CrossLineAxis::Horizontal;
        self
    }

    pub fn both(mut self) -> Self {
        self.direction = CrossLineAxis::Both;
        self
    }

    pub fn height(mut self, height: f32) -> Self {
        self.vertical.1 = Some(height);
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.horizontal.1 = Some(width);
        self
    }

    /// Confines the vertical line to `[start, start + length]`, keeping it within the plot area.
    pub fn span(mut self, start: f32, length: f32) -> Self {
        self.vertical = (start, Some(length));
        self
    }

    /// Confines the horizontal line to `[start, start + length]`.
    pub fn h_span(mut self, start: f32, length: f32) -> Self {
        self.horizontal = (start, Some(length));
        self
    }
}

impl From<Point<Pixels>> for CrossLine {
    fn from(value: Point<Pixels>) -> Self {
        Self::new(value)
    }
}

impl CrossLine {
    fn line(&self, vertical: bool, cx: &App) -> Div {
        let color = if self.dashed {
            cx.theme().border.mix(cx.theme().foreground, 0.8)
        } else {
            cx.theme().foreground.opacity(0.08)
        };
        // Dashed hairline is a zero-width strip drawn entirely by its border.
        let thickness = if self.dashed { px(0.) } else { self.thickness };
        let (start, length) = if vertical {
            self.vertical
        } else {
            self.horizontal
        };

        let el = div().absolute();
        let el = if vertical {
            el.left(self.point.x - thickness * 0.5)
                .w(thickness)
                .top(px(start))
                .map(|el| match length {
                    Some(length) => el.h(px(length)),
                    None => el.h_full(),
                })
        } else {
            el.top(self.point.y - thickness * 0.5)
                .h(thickness)
                .left(px(start))
                .map(|el| match length {
                    Some(length) => el.w(px(length)),
                    None => el.w_full(),
                })
        };

        if self.dashed {
            let el = if vertical {
                el.border_l_1()
            } else {
                el.border_t_1()
            };
            el.border_dashed().border_color(color)
        } else {
            el.bg(color)
        }
    }
}

impl RenderOnce for CrossLine {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let vertical = self.direction.show_vertical().then(|| self.line(true, cx));
        let horizontal = self
            .direction
            .show_horizontal()
            .then(|| self.line(false, cx));

        div()
            .size_full()
            .absolute()
            .top_0()
            .left_0()
            .children(vertical)
            .children(horizontal)
    }
}

#[derive(IntoElement)]
pub struct Dot {
    point: Point<Pixels>,
    size: Pixels,
    stroke: Hsla,
    fill: Hsla,
}

impl Dot {
    pub fn new(point: Point<Pixels>) -> Self {
        Self {
            point,
            size: px(6.),
            stroke: gpui::transparent_black(),
            fill: gpui::transparent_black(),
        }
    }

    pub fn size(mut self, size: impl Into<Pixels>) -> Self {
        self.size = size.into();
        self
    }

    pub fn stroke(mut self, stroke: Hsla) -> Self {
        self.stroke = stroke;
        self
    }

    pub fn fill(mut self, fill: Hsla) -> Self {
        self.fill = fill;
        self
    }
}

impl RenderOnce for Dot {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let border_width = px(1.);
        let offset = self.size / 2. - border_width / 2.;

        div()
            .absolute()
            .w(self.size)
            .h(self.size)
            .rounded_full()
            .border(border_width)
            .border_color(self.stroke)
            .bg(self.fill)
            .left(self.point.x - offset)
            .top(self.point.y - offset)
    }
}

#[derive(Clone)]
pub struct TooltipState {
    pub index: usize,
    pub cross_line: Point<Pixels>,
    pub dots: Vec<Point<Pixels>>,
}

impl TooltipState {
    pub fn new(index: usize, cross_line: Point<Pixels>, dots: Vec<Point<Pixels>>) -> Self {
        Self {
            index,
            cross_line,
            dots,
        }
    }
}

struct TooltipRow {
    color: Hsla,
    label: SharedString,
    value: SharedString,
}

#[derive(IntoElement)]
pub struct Tooltip {
    base: Div,
    gap: Pixels,
    cross_line: Option<CrossLine>,
    dots: Option<Vec<Dot>>,
    appearance: bool,
    title: Option<SharedString>,
    rows: Vec<TooltipRow>,
    cursor: Point<Pixels>,
    /// Used to flip the box toward the center near each edge so it never overflows.
    within: Size<Pixels>,
}

impl Tooltip {
    pub fn new(cursor: Point<Pixels>, within: Size<Pixels>) -> Self {
        Self {
            base: v_flex(),
            gap: px(0.),
            cross_line: None,
            dots: None,
            appearance: true,
            title: None,
            rows: Vec::new(),
            cursor,
            within,
        }
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn row(
        mut self,
        color: impl Into<Hsla>,
        label: impl Into<SharedString>,
        value: impl Into<SharedString>,
    ) -> Self {
        self.rows.push(TooltipRow {
            color: color.into(),
            label: label.into(),
            value: value.into(),
        });
        self
    }

    pub fn gap(mut self, gap: impl Into<Pixels>) -> Self {
        self.gap = gap.into();
        self
    }

    pub fn cross_line(mut self, cross_line: CrossLine) -> Self {
        self.cross_line = Some(cross_line);
        self
    }

    pub fn dots(mut self, dots: impl IntoIterator<Item = Dot>) -> Self {
        self.dots = Some(dots.into_iter().collect());
        self
    }

    pub fn appearance(mut self, appearance: bool) -> Self {
        self.appearance = appearance;
        self
    }
}

impl Styled for Tooltip {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl ParentElement for Tooltip {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.base.extend(elements);
    }
}

impl RenderOnce for Tooltip {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let Tooltip {
            base,
            gap,
            cross_line,
            dots,
            appearance,
            title,
            rows,
            cursor,
            within,
        } = self;

        // Structured content takes precedence over freeform `base` children.
        let content = if title.is_some() || !rows.is_empty() {
            v_flex()
                .text_sm()
                .gap_1()
                .when_some(title, |this, title| {
                    this.child(div().font_semibold().child(title))
                })
                .children(rows.into_iter().map(|row| {
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1p5()
                                .child(div().size_2().rounded_sm().bg(row.color))
                                .child(
                                    div()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(row.label),
                                ),
                        )
                        .child(div().child(row.value))
                }))
        } else {
            base
        };

        div()
            .size_full()
            .absolute()
            .top_0()
            .left_0()
            .when_some(cross_line, |this, cross_line| this.child(cross_line))
            .when_some(dots, |this, dots| this.children(dots))
            // Only the box is deferred: it can overflow plot bounds and must paint above sibling content.
            .child(deferred(content.map(|mut this| {
                if !appearance {
                    return this.size_full().relative();
                }

                let min_w_unset = this.style().min_size.width.is_none();

                this.absolute()
                    .when(min_w_unset, |c| c.min_w(px(150.)))
                    .popover_style(cx)
                    .p_2()
                    .map(|c| {
                        if cursor.x < within.width * 0.5 {
                            c.left(cursor.x + gap)
                        } else {
                            c.right(within.width - cursor.x + gap)
                        }
                    })
                    .map(|c| {
                        if cursor.y < within.height * 0.5 {
                            c.top(cursor.y + gap)
                        } else {
                            c.bottom(within.height - cursor.y + gap)
                        }
                    })
            })))
    }
}
