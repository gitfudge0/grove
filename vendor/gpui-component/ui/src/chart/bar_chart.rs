use std::{ops::RangeInclusive, rc::Rc};

use gpui::{
    AnyElement, App, Background, Bounds, Corners, ElementId, Hsla, IntoElement, LinearColorStop,
    Pixels, Point, SharedString, Size, TextAlign, Window, linear_gradient, point, px,
};
use gpui_component_macros::IntoPlot;
use num_traits::{Num, ToPrimitive};

use crate::{
    ActiveTheme,
    plot::{
        AXIS_GAP, AxisLabelSide, Grid, Plot, PlotAxis,
        label::{TEXT_GAP, TEXT_SIZE, Text, measure_text_width},
        scale::{Scale, ScaleBand, ScaleLinear, Sealed},
        shape::{Bar, BarAlignment},
        tooltip::{CrossLine, Tooltip, TooltipState},
    },
};

use super::build_band_labels;

#[derive(IntoPlot)]
pub struct BarChart<T, B, V>
where
    T: 'static,
    B: PartialEq + Into<SharedString> + 'static,
    V: Copy + PartialOrd + Num + ToPrimitive + Sealed + 'static,
{
    data: Vec<T>,
    band: Option<Rc<dyn Fn(&T) -> B>>,
    value: Option<Rc<dyn Fn(&T) -> V>>,
    fill: Option<Rc<dyn Fn(&T, Bounds<f32>, Bounds<f32>, BarAlignment) -> Background>>,
    #[allow(clippy::type_complexity)]
    fill_gradient:
        Option<Rc<dyn Fn(&T, RangeInclusive<f32>, &dyn Fn(f32) -> f32) -> [LinearColorStop; 2]>>,
    tick_margin: usize,
    label: Option<Rc<dyn Fn(&T) -> SharedString>>,
    label_axis: bool,
    grid: bool,
    alignment: BarAlignment,
    corner_radii: Corners<Pixels>,
    id: Option<ElementId>,
    name: Option<SharedString>,
}

impl<T, B, V> BarChart<T, B, V>
where
    B: PartialEq + Into<SharedString> + 'static,
    V: Copy + PartialOrd + Num + ToPrimitive + Sealed + 'static,
{
    pub fn new<I>(data: I) -> Self
    where
        I: IntoIterator<Item = T>,
    {
        Self {
            data: data.into_iter().collect(),
            band: None,
            value: None,
            fill: None,
            fill_gradient: None,
            tick_margin: 1,
            label: None,
            label_axis: true,
            grid: true,
            alignment: BarAlignment::default(),
            corner_radii: Corners::all(px(0.)),
            id: None,
            name: None,
        }
    }

    /// Enables an interactive hover tooltip. `id` must be unique among sibling elements, or the chart stays non-interactive.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn band(mut self, band: impl Fn(&T) -> B + 'static) -> Self {
        self.band = Some(Rc::new(band));
        self
    }

    pub fn value(mut self, value: impl Fn(&T) -> V + 'static) -> Self {
        self.value = Some(Rc::new(value));
        self
    }

    /// Closure receives the datum, the bar's and chart's bounds (both pixel-space), and the alignment; clears [`BarChart::fill_gradient`].
    pub fn fill<Bg>(
        mut self,
        fill: impl Fn(&T, Bounds<f32>, Bounds<f32>, BarAlignment) -> Bg + 'static,
    ) -> Self
    where
        Bg: Into<Background> + 'static,
    {
        self.fill = Some(Rc::new(move |t, bar_bounds, chart_bounds, alignment| {
            fill(t, bar_bounds, chart_bounds, alignment).into()
        }));
        self.fill_gradient = None;
        self
    }

    /// Closure receives the datum, the chart's data range, and a `chart_to_bar` remap helper
    /// mapping a chart-value coordinate to a bar-local gradient position (0.0 = base, 1.0 = tip).
    ///
    /// ```ignore
    /// .fill_gradient(|_, chart_range, chart_to_bar| [
    ///     linear_color_stop(c.opacity(0.3), chart_to_bar(*chart_range.start())),
    ///     linear_color_stop(c,              chart_to_bar(*chart_range.end())),
    /// ])
    /// ```
    ///
    /// Stops outside `[0, 1]` are clipped to the bar with interpolated colors. Clears any
    /// previously set [`BarChart::fill`].
    pub fn fill_gradient(
        mut self,
        fill: impl Fn(&T, RangeInclusive<f32>, &dyn Fn(f32) -> f32) -> [LinearColorStop; 2] + 'static,
    ) -> Self {
        self.fill_gradient = Some(Rc::new(fill));
        self.fill = None;
        self
    }

    pub fn tick_margin(mut self, tick_margin: usize) -> Self {
        self.tick_margin = tick_margin;
        self
    }

    pub fn label<S>(mut self, label: impl Fn(&T) -> S + 'static) -> Self
    where
        S: Into<SharedString> + 'static,
    {
        self.label = Some(Rc::new(move |t| label(t).into()));
        self
    }

    /// Default is true.
    pub fn label_axis(mut self, label_axis: bool) -> Self {
        self.label_axis = label_axis;
        self
    }

    pub fn grid(mut self, grid: bool) -> Self {
        self.grid = grid;
        self
    }

    /// Default is [`BarAlignment::Bottom`].
    pub fn alignment(mut self, alignment: BarAlignment) -> Self {
        self.alignment = alignment;
        self
    }

    pub fn corner_radii(mut self, corner_radii: impl Into<Corners<Pixels>>) -> Self {
        self.corner_radii = corner_radii.into();
        self
    }

    /// Spans height for horizontal bars, width otherwise. Shared by `tooltip_state` and `tooltip`.
    fn band_scale(&self, bounds: Bounds<Pixels>) -> Option<ScaleBand<B>> {
        let band_fn = self.band.as_ref()?;
        let band_extent = if self.alignment.is_horizontal() {
            bounds.size.height.as_f32()
        } else {
            bounds.size.width.as_f32()
        };
        Some(
            ScaleBand::new(
                self.data.iter().map(|v| band_fn(v)).collect(),
                vec![0., band_extent],
            )
            .padding_inner(0.4)
            .padding_outer(0.2),
        )
    }

    /// `(band_side, value_end_side)` gaps measured from actual label text; shared by `paint` and the tooltip.
    fn horizontal_gaps(&self, window: &mut Window) -> (f32, f32) {
        let Some(band_fn) = self.band.as_ref() else {
            return (0., 0.);
        };
        let font_size = px(TEXT_SIZE);
        let band_gap = if self.label_axis {
            self.data
                .iter()
                .map(|v| {
                    let s: SharedString = band_fn(v).into();
                    measure_text_width(&s, font_size, window)
                })
                .fold(0f32, f32::max)
                + TEXT_GAP * 2.
        } else {
            0.
        };
        let value_end_gap = if let Some(label_fn) = self.label.as_ref() {
            self.data
                .iter()
                .map(|v| measure_text_width(&label_fn(v), font_size, window))
                .fold(0f32, f32::max)
                + TEXT_GAP * 2.
        } else {
            TEXT_GAP * 4.
        };
        (band_gap, value_end_gap)
    }
}

impl<T, B, V> Plot for BarChart<T, B, V>
where
    B: PartialEq + Into<SharedString> + 'static,
    V: Copy + PartialOrd + Num + ToPrimitive + Sealed + 'static,
{
    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let (Some(band_fn), Some(value_fn)) = (self.band.as_ref(), self.value.as_ref()) else {
            return;
        };

        let total_width = bounds.size.width.as_f32();
        let total_height = bounds.size.height.as_f32();
        let axis_gap = if self.label_axis { AXIS_GAP } else { 0. };
        let alignment = self.alignment;
        let is_horizontal = alignment.is_horizontal();

        let Some(band_scale) = self.band_scale(bounds) else {
            return;
        };
        let band_width = band_scale.band_width();

        let value_dim = if is_horizontal {
            total_width
        } else {
            total_height
        };
        // Horizontal charts measure actual label width (can be arbitrarily wide) instead of a fixed constant.
        let (band_gap, value_end_gap) = if is_horizontal {
            self.horizontal_gaps(window)
        } else {
            (axis_gap, 10.)
        };
        let (range, baseline) = match alignment {
            BarAlignment::Bottom => {
                let baseline = value_dim - axis_gap;
                (vec![baseline, 10.], baseline)
            }
            BarAlignment::Top => {
                let baseline = axis_gap;
                (vec![baseline, value_dim - 10.], baseline)
            }
            BarAlignment::Left => {
                let baseline = band_gap;
                (vec![baseline, value_dim - value_end_gap], baseline)
            }
            BarAlignment::Right => {
                let baseline = value_dim - band_gap;
                (vec![baseline, value_end_gap], baseline)
            }
        };
        let value_scale = ScaleLinear::new(
            self.data
                .iter()
                .map(|v| value_fn(v))
                .chain(Some(V::zero()))
                .collect(),
            range,
        );

        let mut axis = PlotAxis::new().stroke(cx.theme().border);
        if self.label_axis {
            let labels = build_band_labels(
                &self.data,
                band_fn.as_ref(),
                &band_scale,
                band_width,
                self.tick_margin,
                cx.theme().muted_foreground,
            );
            axis = match alignment {
                BarAlignment::Bottom => axis.x(baseline).x_label(labels),
                BarAlignment::Top => axis
                    .x(baseline)
                    .x_label_side(AxisLabelSide::Start)
                    .x_label(labels),
                BarAlignment::Left => axis
                    .y(baseline)
                    .y_label_side(AxisLabelSide::Start)
                    .y_label(labels.into_iter().map(|t| t.align(TextAlign::Right))),
                BarAlignment::Right => axis
                    .y(baseline)
                    .y_label(labels.into_iter().map(|t| t.align(TextAlign::Left))),
            };
        }
        axis.paint(&bounds, window, cx);

        // Far edge of the value axis, opposite the baseline.
        let far = match alignment {
            BarAlignment::Bottom => 10.,
            BarAlignment::Top => value_dim - 10.,
            BarAlignment::Left => value_dim - value_end_gap,
            BarAlignment::Right => value_end_gap,
        };

        if self.grid {
            let grid_steps: Vec<f32> = (0..4)
                .map(|i| far + (baseline - far) * i as f32 / 4.0)
                .collect();
            let grid = Grid::new()
                .stroke(cx.theme().border)
                .dash_array(&[px(4.), px(2.)]);
            let grid = if is_horizontal {
                grid.x(grid_steps)
            } else {
                grid.y(grid_steps)
            };
            grid.paint(&bounds, window);
        }

        let band_fn_cloned = band_fn.clone();
        let value_fn_cloned = value_fn.clone();
        let default_fill: Background = cx.theme().chart_2.into();
        let fill = self.fill.clone();
        let fill_gradient = self.fill_gradient.clone();
        let label_color = cx.theme().foreground;

        // Passed to user fill closures so they can position chart-wide backgrounds.
        let chart_bounds: Bounds<f32> = Bounds {
            origin: Point::new(0., 0.),
            size: Size::new(total_width, total_height),
        };

        // Passed to fill_gradient callers and used by the chart_to_bar remap helper.
        let chart_range = {
            let mut lo = 0.0_f32;
            let mut hi = 0.0_f32;
            for v in &self.data {
                if let Some(f) = value_fn(v).to_f32() {
                    lo = lo.min(f);
                    hi = hi.max(f);
                }
            }
            lo..=hi
        };

        let mut bar = Bar::new()
            .data(&self.data)
            .alignment(alignment)
            .band_width(band_width)
            .cross(move |d| band_scale.tick(&band_fn_cloned(d)))
            .base(move |_| baseline)
            .value(move |d| value_scale.tick(&value_fn_cloned(d)))
            .corner_radii(self.corner_radii);

        bar = match (fill, fill_gradient) {
            (_, Some(fg)) => {
                let value_fn_for_grad = value_fn.clone();
                bar.fill(move |d, _frame, alignment| {
                    let v = value_fn_for_grad(d).to_f32().unwrap_or(0.);
                    let base_v = 0.0_f32;
                    let bar_lo = base_v.min(v);
                    let bar_hi = base_v.max(v);
                    let bar_span = (bar_hi - bar_lo).max(f32::EPSILON);
                    let chart_to_bar = |chart_value: f32| (chart_value - bar_lo) / bar_span;
                    let stops = fg(d, chart_range.clone(), &chart_to_bar);
                    let [s0, s1] = clip_stops_to_bar(stops);
                    let bg: Background = linear_gradient(alignment.gradient_angle(), s0, s1);
                    bg
                })
            }
            (Some(f), _) => {
                bar.fill(move |d, frame, alignment| f(d, frame, chart_bounds, alignment))
            }
            _ => bar.fill(move |_, _, _| default_fill),
        };

        if let Some(label) = self.label.as_ref() {
            let label = label.clone();
            let text_align = match alignment {
                BarAlignment::Bottom | BarAlignment::Top => TextAlign::Center,
                BarAlignment::Left => TextAlign::Left,
                BarAlignment::Right => TextAlign::Right,
            };
            bar =
                bar.label(move |d, p| vec![Text::new(label(d), p, label_color).align(text_align)]);
        }

        bar.paint(&bounds, window, cx);
    }

    fn id(&self) -> Option<ElementId> {
        self.id.clone()
    }

    fn tooltip_state(
        &self,
        position: Point<Pixels>,
        bounds: Bounds<Pixels>,
        _cx: &App,
    ) -> Option<TooltipState> {
        let band_fn = self.band.as_ref()?;
        self.value.as_ref()?;

        let is_horizontal = self.alignment.is_horizontal();
        let band_scale = self.band_scale(bounds)?;
        let band_width = band_scale.band_width();

        let cursor_band = if is_horizontal {
            position.y
        } else {
            position.x
        };
        let index = band_scale.least_index(cursor_band.as_f32());
        let d = self.data.get(index)?;
        let center = band_scale.tick(&band_fn(d))? + band_width / 2.;

        let cross_line = if is_horizontal {
            point(position.x, px(center))
        } else {
            point(px(center), position.y)
        };

        Some(TooltipState::new(index, cross_line, vec![]))
    }

    fn tooltip(
        &self,
        state: &TooltipState,
        cursor: Point<Pixels>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let (band_fn, value_fn) = (self.band.as_ref()?, self.value.as_ref()?);
        let d = self.data.get(state.index)?;
        let title: SharedString = band_fn(d).into();
        let value = value_fn(d).to_f64()?;
        let name = self.name.clone().unwrap_or_default();

        let band_width = self.band_scale(bounds)?.band_width();
        let cross_line = if self.alignment.is_horizontal() {
            let (band_gap, value_end_gap) = self.horizontal_gaps(window);
            let length = (bounds.size.width.as_f32() - band_gap - value_end_gap).max(0.);
            let start = if matches!(self.alignment, BarAlignment::Left) {
                band_gap
            } else {
                value_end_gap
            };
            if cursor.x.as_f32() < start || cursor.x.as_f32() > start + length {
                return None;
            }
            CrossLine::new(state.cross_line)
                .horizontal()
                .h_span(start, length)
                .band(px(band_width))
        } else {
            let axis_gap = if self.label_axis { AXIS_GAP } else { 0. };
            let length = bounds.size.height.as_f32() - axis_gap;
            let start = if matches!(self.alignment, BarAlignment::Top) {
                axis_gap
            } else {
                0.
            };
            if cursor.y.as_f32() < start || cursor.y.as_f32() > start + length {
                return None;
            }
            CrossLine::new(state.cross_line)
                .span(start, length)
                .band(px(band_width))
        };

        Some(
            Tooltip::new(cursor, bounds.size)
                .gap(px(8.))
                .cross_line(cross_line)
                .title(title)
                .row(cx.theme().chart_2, name, format!("{}", value))
                .into_any_element(),
        )
    }
}

/// gpui would clamp an out-of-range stop and lose the gradient effect; sample the color at 0.0/1.0 along the line instead.
fn clip_stops_to_bar(stops: [LinearColorStop; 2]) -> [LinearColorStop; 2] {
    let [a, b] = stops;
    let p0 = a.percentage;
    let p1 = b.percentage;
    let lerp = |t: f32| -> Hsla {
        Hsla {
            h: a.color.h + (b.color.h - a.color.h) * t,
            s: a.color.s + (b.color.s - a.color.s) * t,
            l: a.color.l + (b.color.l - a.color.l) * t,
            a: a.color.a + (b.color.a - a.color.a) * t,
        }
    };
    let span = p1 - p0;
    let sample = |target: f32| -> Hsla {
        if span.abs() < f32::EPSILON {
            a.color
        } else {
            lerp((target - p0) / span)
        }
    };
    let new_a = if (0. ..=1.).contains(&p0) {
        a
    } else {
        LinearColorStop {
            color: sample(p0.clamp(0., 1.)),
            percentage: p0.clamp(0., 1.),
        }
    };
    let new_b = if (0. ..=1.).contains(&p1) {
        b
    } else {
        LinearColorStop {
            color: sample(p1.clamp(0., 1.)),
            percentage: p1.clamp(0., 1.),
        }
    };
    [new_a, new_b]
}
