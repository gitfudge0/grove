use std::{
    f32::consts::{PI, TAU},
    rc::Rc,
};

use gpui::{
    AnyElement, App, AvailableSpace, Background, Bounds, ElementId, Hsla, IntoElement, Pixels,
    Point, SharedString, TextAlign, Window, point, px,
};
use gpui_component_macros::IntoPlot;
use num_traits::{Num, ToPrimitive, Zero};

use crate::{
    ActiveTheme,
    plot::{
        Plot,
        label::{PlotLabel, TEXT_SIZE, Text},
        polygon,
        scale::{Scale, ScaleLinear, Sealed},
        shape::RadialLine,
        tooltip::{Dot, Tooltip, TooltipState},
    },
};

const HALF_PI: f32 = PI / 2.;
const DEFAULT_LABEL_GAP: f32 = 10.;
const DEFAULT_GRID_LEVELS: usize = 4;

/// The label of one radar dimension, returned by [`RadarChart::label`].
pub enum RadarLabel {
    /// Honors [`RadarChart::label_color`] and supplies the tooltip title.
    Text(SharedString),
    /// Measured at its natural size; `label_color` does not apply and it supplies no tooltip title.
    Element(AnyElement),
}

impl From<&'static str> for RadarLabel {
    fn from(text: &'static str) -> Self {
        Self::Text(text.into())
    }
}

impl From<String> for RadarLabel {
    fn from(text: String) -> Self {
        Self::Text(text.into())
    }
}

impl From<SharedString> for RadarLabel {
    fn from(text: SharedString) -> Self {
        Self::Text(text)
    }
}

impl From<AnyElement> for RadarLabel {
    fn from(element: AnyElement) -> Self {
        Self::Element(element)
    }
}

/// A radar (spider) chart.
///
/// Each datum is one dimension (a spoke), placed clockwise around the center
/// starting at 12 o'clock. Add one series per [`RadarChart::value`] call; each
/// series is drawn as a closed polygon connecting its values on every spoke.
#[derive(IntoPlot)]
pub struct RadarChart<T, Y>
where
    T: 'static,
    Y: Clone + Copy + PartialOrd + Num + ToPrimitive + Sealed + 'static,
{
    data: Vec<T>,
    values: Vec<Rc<dyn Fn(&T) -> Y>>,
    strokes: Vec<Hsla>,
    fills: Vec<Background>,
    names: Vec<SharedString>,
    label: Option<Rc<dyn Fn(&T) -> RadarLabel + 'static>>,
    /// Resolved once per frame in `prepaint`; element labels leave `None`.
    label_texts: Vec<Option<SharedString>>,
    label_color: Option<Hsla>,
    label_gap: f32,
    max_value: Option<Y>,
    outer_radius: f32,
    grid: bool,
    grid_levels: usize,
    dot: bool,
    id: Option<ElementId>,
}

impl<T, Y> RadarChart<T, Y>
where
    Y: Clone + Copy + PartialOrd + Num + ToPrimitive + Sealed + 'static,
{
    pub fn new<I>(data: I) -> Self
    where
        I: IntoIterator<Item = T>,
    {
        Self {
            data: data.into_iter().collect(),
            values: vec![],
            strokes: vec![],
            fills: vec![],
            names: vec![],
            label: None,
            label_texts: vec![],
            label_color: None,
            label_gap: DEFAULT_LABEL_GAP,
            max_value: None,
            outer_radius: 0.,
            grid: true,
            grid_levels: DEFAULT_GRID_LEVELS,
            dot: false,
            id: None,
        }
    }

    /// Without a unique `id`, the chart stays a non-interactive plot.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Call after the matching [`RadarChart::value`].
    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.names.push(name.into());
        self
    }

    /// Call multiple times to overlay multiple series, each paired with matching [`RadarChart::stroke`]/[`RadarChart::fill`] calls.
    pub fn value(mut self, value: impl Fn(&T) -> Y + 'static) -> Self {
        self.values.push(Rc::new(value));
        self
    }

    /// Defaults to the theme chart colors, cycled per series.
    pub fn stroke(mut self, stroke: impl Into<Hsla>) -> Self {
        self.strokes.push(stroke.into());
        self
    }

    /// Defaults to the series stroke color with 0.3 opacity.
    pub fn fill(mut self, fill: impl Into<Background>) -> Self {
        self.fills.push(fill.into());
        self
    }

    /// See [`RadarLabel`] for how a plain string vs. an `into_any_element()` differ.
    ///
    /// ```ignore
    /// RadarChart::new(data).label(|d| d.month.clone())
    ///
    /// RadarChart::new(data).label(|d| {
    ///     v_flex()
    ///         .items_center()
    ///         .child(Icon::new(IconName::Star).xsmall())
    ///         .child(d.month.clone())
    ///         .into_any_element()
    /// })
    /// ```
    pub fn label<L>(mut self, label: impl Fn(&T) -> L + 'static) -> Self
    where
        L: Into<RadarLabel> + 'static,
    {
        self.label = Some(Rc::new(move |d| label(d).into()));
        self
    }

    /// Defaults to `cx.theme().muted_foreground`; element labels style themselves.
    pub fn label_color(mut self, color: impl Into<Hsla>) -> Self {
        self.label_color = Some(color.into());
        self
    }

    /// Defaults to 10px.
    pub fn label_gap(mut self, gap: f32) -> Self {
        self.label_gap = gap;
        self
    }

    /// Defaults to the maximum value across all series.
    pub fn max_value(mut self, max_value: Y) -> Self {
        self.max_value = Some(max_value);
        self
    }

    /// Defaults to 40% of the bounds height.
    pub fn outer_radius(mut self, outer_radius: f32) -> Self {
        self.outer_radius = outer_radius;
        self
    }

    pub fn grid(mut self, grid: bool) -> Self {
        self.grid = grid;
        self
    }

    pub fn grid_levels(mut self, grid_levels: usize) -> Self {
        self.grid_levels = grid_levels.max(1);
        self
    }

    pub fn dot(mut self) -> Self {
        self.dot = true;
        self
    }

    fn series_stroke(&self, ix: usize, cx: &App) -> Hsla {
        let colors = [
            cx.theme().chart_1,
            cx.theme().chart_2,
            cx.theme().chart_3,
            cx.theme().chart_4,
            cx.theme().chart_5,
        ];

        self.strokes
            .get(ix)
            .copied()
            .unwrap_or(colors[ix % colors.len()])
    }

    fn resolve_outer_radius(&self, bounds: &Bounds<Pixels>) -> f32 {
        if self.outer_radius.is_zero() {
            bounds.size.height.as_f32() * 0.4
        } else {
            self.outer_radius
        }
    }

    /// Anchor point plus outward radial unit vector; shared by `prepaint`/`paint` so element and text labels land in the same place.
    fn label_anchor(
        &self,
        ix: usize,
        outer_radius: f32,
        bounds: &Bounds<Pixels>,
    ) -> (Point<f32>, Point<f32>) {
        let label_radius = outer_radius + self.label_gap;
        let angle = ix as f32 * TAU / self.data.len() as f32 - HALF_PI;
        let direction = point(angle.cos(), angle.sin());

        let anchor = point(
            bounds.size.width.as_f32() / 2. + label_radius * direction.x,
            bounds.size.height.as_f32() / 2. + label_radius * direction.y,
        );

        (anchor, direction)
    }

    /// Domain includes zero so non-negative data starts at the center; shared by `paint`/`tooltip_state` to stay in sync.
    fn scale(&self, outer_radius: f32) -> ScaleLinear<Y> {
        let domain = if let Some(max_value) = self.max_value {
            vec![Y::zero(), max_value]
        } else {
            self.data
                .iter()
                .flat_map(|d| self.values.iter().map(|value_fn| value_fn(d)))
                .chain(Some(Y::zero()))
                .collect()
        };

        ScaleLinear::new(domain, vec![0., outer_radius])
    }

    fn hovered_index(&self, position: Point<Pixels>, bounds: Bounds<Pixels>) -> Option<usize> {
        let n = self.data.len();
        if n == 0 {
            return None;
        }

        let outer_radius = self.resolve_outer_radius(&bounds);
        let dx = position.x.as_f32() - bounds.size.width.as_f32() / 2.;
        let dy = position.y.as_f32() - bounds.size.height.as_f32() / 2.;
        if dx.hypot(dy) > outer_radius + self.label_gap {
            return None;
        }

        // Screen angle -> chart angle (0 at 12 o'clock, clockwise).
        let angle = (dy.atan2(dx) + HALF_PI).rem_euclid(TAU);
        Some((angle * n as f32 / TAU).round() as usize % n)
    }
}

impl<T, Y> Plot for RadarChart<T, Y>
where
    Y: Clone + Copy + PartialOrd + Num + ToPrimitive + Sealed + 'static,
{
    /// Measuring is illegal in `paint`, so element labels are laid out here.
    fn prepaint(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        self.label_texts.clear();

        let n = self.data.len();
        if n == 0 || self.values.is_empty() {
            return vec![];
        }
        let Some(label_fn) = self.label.clone() else {
            return vec![];
        };

        let outer_radius = self.resolve_outer_radius(&bounds);
        let mut texts = Vec::with_capacity(n);
        let mut elements = vec![];

        for (ix, d) in self.data.iter().enumerate() {
            match label_fn(d) {
                RadarLabel::Text(text) => texts.push(Some(text)),
                RadarLabel::Element(mut element) => {
                    texts.push(None);

                    let size = element.layout_as_root(AvailableSpace::min_size(), window, cx);
                    let (anchor, direction) = self.label_anchor(ix, outer_radius, &bounds);

                    // Pushes the box radially outward so a tall label clears the ring instead of straddling it.
                    let origin = bounds.origin
                        + point(
                            px(anchor.x + (direction.x - 1.) * size.width.as_f32() / 2.),
                            px(anchor.y + (direction.y - 1.) * size.height.as_f32() / 2.),
                        );

                    element.prepaint_at(origin, window, cx);
                    elements.push(element);
                }
            }
        }

        self.label_texts = texts;

        elements
    }

    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let n = self.data.len();
        if n == 0 || self.values.is_empty() {
            return;
        }

        let outer_radius = self.resolve_outer_radius(&bounds);
        let angle_step = TAU / n as f32;
        let center_x = bounds.size.width.as_f32() / 2.;
        let center_y = bounds.size.height.as_f32() / 2.;
        let scale = self.scale(outer_radius);

        if self.grid {
            let stroke = cx.theme().border;

            for level in 1..=self.grid_levels {
                let radius = outer_radius * level as f32 / self.grid_levels as f32;
                RadialLine::new()
                    .data(0..n)
                    .angle(move |_, i| Some(i as f32 * angle_step))
                    .radius(move |_, _| Some(radius))
                    .closed()
                    .stroke(stroke)
                    .paint(&bounds, window);
            }

            for i in 0..n {
                let angle = i as f32 * angle_step - HALF_PI;
                let points = [
                    point(center_x, center_y),
                    point(
                        center_x + outer_radius * angle.cos(),
                        center_y + outer_radius * angle.sin(),
                    ),
                ];
                if let Some(path) = polygon(&points, &bounds) {
                    window.paint_path(path, stroke);
                }
            }
        }

        for (i, value_fn) in self.values.iter().enumerate() {
            let stroke = self.series_stroke(i, cx);
            let fill = self
                .fills
                .get(i)
                .copied()
                .unwrap_or_else(|| stroke.opacity(0.3).into());

            let scale = scale.clone();
            let value_fn = value_fn.clone();
            let mut line = RadialLine::new()
                .data(&self.data)
                .angle(move |_, i| Some(i as f32 * angle_step))
                .radius(move |d, _| scale.tick(&value_fn(d)))
                .closed()
                .fill(fill)
                .stroke(stroke)
                .stroke_width(2.);
            if self.dot {
                line = line.dot().dot_size(8.).dot_fill_color(stroke);
            }
            line.paint(&bounds, window);
        }

        let label_color = self.label_color.unwrap_or(cx.theme().muted_foreground);
        let labels = self
            .label_texts
            .iter()
            .enumerate()
            .filter_map(|(ix, text)| {
                let text = text.clone()?;
                let (anchor, direction) = self.label_anchor(ix, outer_radius, &bounds);

                // Epsilon absorbs float noise at 12/6 o'clock.
                let align = if direction.x > 1e-3 {
                    TextAlign::Left
                } else if direction.x < -1e-3 {
                    TextAlign::Right
                } else {
                    TextAlign::Center
                };

                Some(
                    Text::new(
                        text,
                        point(px(anchor.x), px(anchor.y - TEXT_SIZE / 2.)),
                        label_color,
                    )
                    .align(align),
                )
            });

        PlotLabel::new(labels.collect()).paint(&bounds, window, cx);
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
        if self.values.is_empty() {
            return None;
        }
        let index = self.hovered_index(position, bounds)?;
        let d = self.data.get(index)?;

        let outer_radius = self.resolve_outer_radius(&bounds);
        let scale = self.scale(outer_radius);
        let center_x = bounds.size.width.as_f32() / 2.;
        let center_y = bounds.size.height.as_f32() / 2.;
        let angle = index as f32 * TAU / self.data.len() as f32 - HALF_PI;

        let dots = self
            .values
            .iter()
            .filter_map(|value_fn| {
                let radius = scale.tick(&value_fn(d))?;
                Some(point(
                    px(center_x + radius * angle.cos()),
                    px(center_y + radius * angle.sin()),
                ))
            })
            .collect();

        Some(TooltipState::new(index, position, dots))
    }

    fn tooltip(
        &self,
        state: &TooltipState,
        cursor: Point<Pixels>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let d = self.data.get(state.index)?;

        let dot_stroke = cx.theme().background;

        // No crosshair: a radar has no cartesian axis to snap to.
        let mut tooltip =
            Tooltip::new(cursor, bounds.size)
                .gap(px(8.))
                .dots(state.dots.iter().enumerate().map(|(i, p)| {
                    Dot::new(*p)
                        .stroke(dot_stroke)
                        .fill(self.series_stroke(i, cx))
                }));

        if let Some(title) = self.label_texts.get(state.index).cloned().flatten() {
            tooltip = tooltip.title(title);
        }

        for (i, value_fn) in self.values.iter().enumerate() {
            let name = self.names.get(i).cloned().unwrap_or_default();
            let value = value_fn(d).to_f64()?;
            tooltip = tooltip.row(self.series_stroke(i, cx), name, format!("{}", value));
        }

        Some(tooltip.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct Item {
        subject: SharedString,
        a: f64,
        b: f64,
    }

    #[test]
    fn test_radar_chart_builder() {
        let data = vec![
            Item {
                subject: "Sales".into(),
                a: 80.,
                b: 60.,
            },
            Item {
                subject: "Marketing".into(),
                a: 50.,
                b: 90.,
            },
        ];

        let chart = RadarChart::new(data.clone())
            .label(|d| d.subject.clone())
            .value(|d| d.a)
            .stroke(gpui::red())
            .fill(gpui::red())
            .name("A")
            .value(|d| d.b)
            .max_value(100.)
            .outer_radius(120.)
            .label_gap(8.)
            .grid(false)
            .grid_levels(5)
            .dot()
            .id("radar");

        assert_eq!(chart.data.len(), 2);
        assert_eq!(chart.values.len(), 2);
        assert_eq!(chart.strokes.len(), 1);
        assert_eq!(chart.fills.len(), 1);
        assert_eq!(chart.names.len(), 1);
        assert!(chart.label.is_some());
        assert_eq!(chart.max_value, Some(100.));
        assert_eq!(chart.outer_radius, 120.);
        assert_eq!(chart.label_gap, 8.);
        assert!(!chart.grid);
        assert_eq!(chart.grid_levels, 5);
        assert!(chart.dot);
        assert!(chart.id.is_some());

        let values = (chart.values[0](&data[0]), chart.values[1](&data[0]));
        assert_eq!(values, (80., 60.));
    }

    #[test]
    fn test_radar_label_from_text() {
        let labels = [
            RadarLabel::from("Sales"),
            RadarLabel::from("Sales".to_string()),
            RadarLabel::from(SharedString::from("Sales")),
        ];

        for label in labels {
            assert!(matches!(label, RadarLabel::Text(text) if text == "Sales"));
        }
    }

    #[test]
    fn test_radar_chart_grid_levels_min() {
        let chart: RadarChart<Item, f64> = RadarChart::new(vec![]).grid_levels(0);
        assert_eq!(chart.grid_levels, 1);
    }

    #[test]
    fn test_radar_chart_hovered_index() {
        let data = (0..4)
            .map(|i| Item {
                subject: format!("S{}", i).into(),
                a: 50.,
                b: 50.,
            })
            .collect::<Vec<_>>();

        // 200x200 bounds -> center (100,100), outer radius 80, hover region 90.
        let chart: RadarChart<Item, f64> = RadarChart::new(data).value(|d| d.a);
        let bounds = gpui::Bounds::new(point(px(0.), px(0.)), gpui::size(px(200.), px(200.)));

        assert_eq!(
            chart.hovered_index(point(px(100.), px(30.)), bounds),
            Some(0)
        );
        assert_eq!(
            chart.hovered_index(point(px(170.), px(100.)), bounds),
            Some(1)
        );
        assert_eq!(
            chart.hovered_index(point(px(100.), px(170.)), bounds),
            Some(2)
        );
        assert_eq!(
            chart.hovered_index(point(px(30.), px(100.)), bounds),
            Some(3)
        );

        assert_eq!(
            chart.hovered_index(point(px(110.), px(40.)), bounds),
            Some(0)
        );
        assert_eq!(
            chart.hovered_index(point(px(160.), px(90.)), bounds),
            Some(1)
        );

        assert_eq!(chart.hovered_index(point(px(100.), px(5.)), bounds), None);
        assert_eq!(chart.hovered_index(point(px(5.), px(5.)), bounds), None);
    }
}
