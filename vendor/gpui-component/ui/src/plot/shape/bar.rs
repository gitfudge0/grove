use gpui::{
    App, Background, Bounds, Corners, PaintQuad, Pixels, Point, Size, Window, fill, point, px,
};

use crate::plot::{
    label::{PlotLabel, TEXT_GAP, TEXT_HEIGHT, TEXT_SIZE, Text},
    origin_point,
};

/// Controls both orientation (vertical vs horizontal) and which side the baseline lives on.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarAlignment {
    #[default]
    Bottom,
    Top,
    Left,
    Right,
}

impl BarAlignment {
    pub fn is_horizontal(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }

    pub fn is_vertical(self) -> bool {
        !self.is_horizontal()
    }

    /// gpui convention: `0°` points upward, angles increase clockwise.
    pub fn gradient_angle(self) -> f32 {
        match self {
            Self::Bottom => 0.,
            Self::Top => 180.,
            Self::Left => 90.,
            Self::Right => 270.,
        }
    }
}

#[allow(clippy::type_complexity)]
pub struct Bar<T> {
    data: Vec<T>,
    alignment: BarAlignment,
    cross: Box<dyn Fn(&T) -> Option<f32>>,
    band_width: f32,
    base: Box<dyn Fn(&T) -> f32>,
    value: Box<dyn Fn(&T) -> Option<f32>>,
    fill: Box<dyn Fn(&T, Bounds<f32>, BarAlignment) -> Background>,
    label: Option<Box<dyn Fn(&T, Point<Pixels>) -> Vec<Text>>>,
    corner_radii: Corners<Pixels>,
}

impl<T> Default for Bar<T> {
    fn default() -> Self {
        Self {
            data: Vec::new(),
            alignment: BarAlignment::default(),
            cross: Box::new(|_| None),
            band_width: 0.,
            base: Box::new(|_| 0.),
            value: Box::new(|_| None),
            fill: Box::new(|_, _, _| gpui::black().into()),
            label: None,
            corner_radii: Corners::all(px(0.)),
        }
    }
}

impl<T> Bar<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn data<I>(mut self, data: I) -> Self
    where
        I: IntoIterator<Item = T>,
    {
        self.data = data.into_iter().collect();
        self
    }

    /// Default is [`BarAlignment::Bottom`].
    pub fn alignment(mut self, alignment: BarAlignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// X for vertical alignments, Y for horizontal.
    pub fn cross<F>(mut self, cross: F) -> Self
    where
        F: Fn(&T) -> Option<f32> + 'static,
    {
        self.cross = Box::new(cross);
        self
    }

    pub fn band_width(mut self, band_width: f32) -> Self {
        self.band_width = band_width;
        self
    }

    pub fn base<F>(mut self, base: F) -> Self
    where
        F: Fn(&T) -> f32 + 'static,
    {
        self.base = Box::new(base);
        self
    }

    pub fn value<F>(mut self, value: F) -> Self
    where
        F: Fn(&T) -> Option<f32> + 'static,
    {
        self.value = Box::new(value);
        self
    }

    /// Closure gets the datum, painted frame (raw pixels relative to plot origin), and [`BarAlignment`]; the background is used verbatim, gradient angle not auto-adjusted for orientation.
    pub fn fill<F, B>(mut self, fill: F) -> Self
    where
        F: Fn(&T, Bounds<f32>, BarAlignment) -> B + 'static,
        B: Into<Background>,
    {
        self.fill = Box::new(move |v, frame, alignment| fill(v, frame, alignment).into());
        self
    }

    pub fn label<F>(mut self, label: F) -> Self
    where
        F: Fn(&T, Point<Pixels>) -> Vec<Text> + 'static,
    {
        self.label = Some(Box::new(label));
        self
    }

    /// Use [`Corners::all`] for uniform rounding, or construct manually to round only specific corners.
    pub fn corner_radii(mut self, corner_radii: impl Into<Corners<Pixels>>) -> Self {
        self.corner_radii = corner_radii.into();
        self
    }

    fn path(&self, bounds: &Bounds<Pixels>) -> (Vec<PaintQuad>, PlotLabel) {
        let origin = bounds.origin;
        let mut graph = vec![];
        let mut labels = vec![];

        for v in &self.data {
            let Some(cross) = (self.cross)(v) else {
                continue;
            };
            let Some(value) = (self.value)(v) else {
                continue;
            };
            let base = (self.base)(v);

            let bw = self.band_width;
            let (frame, p1, p2) = if self.alignment.is_vertical() {
                let x0 = cross;
                let x1 = cross + bw;
                let y_min = value.min(base);
                let y_max = value.max(base);
                let frame = Bounds {
                    origin: Point::new(x0, y_min),
                    size: Size::new(x1 - x0, y_max - y_min),
                };
                (
                    frame,
                    origin_point(px(x0), px(y_min), origin),
                    origin_point(px(x1), px(y_max), origin),
                )
            } else {
                let y0 = cross;
                let y1 = cross + bw;
                let x_min = value.min(base);
                let x_max = value.max(base);
                let frame = Bounds {
                    origin: Point::new(x_min, y0),
                    size: Size::new(x_max - x_min, y1 - y0),
                };
                (
                    frame,
                    origin_point(px(x_min), px(y0), origin),
                    origin_point(px(x_max), px(y1), origin),
                )
            };

            let bg = (self.fill)(v, frame, self.alignment);
            graph.push(fill(Bounds::from_corners(p1, p2), bg).corner_radii(self.corner_radii));

            if let Some(label) = &self.label {
                let label_origin = label_origin(self.alignment, cross, base, value, bw);
                labels.extend(label(v, label_origin));
            }
        }

        (graph, PlotLabel::new(labels))
    }

    pub fn paint(&self, bounds: &Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let (graph, labels) = self.path(bounds);
        for quad in graph {
            window.paint_quad(quad);
        }
        labels.paint(bounds, window, cx);
    }
}

/// Positioned outside the bar at the value end; caller chooses the [`gpui::TextAlign`].
fn label_origin(
    alignment: BarAlignment,
    cross: f32,
    base: f32,
    value: f32,
    band_width: f32,
) -> Point<Pixels> {
    match alignment {
        BarAlignment::Bottom => {
            let cx = cross + band_width / 2.;
            if value <= base {
                point(px(cx), px(value - TEXT_HEIGHT))
            } else {
                point(px(cx), px(value + TEXT_GAP))
            }
        }
        BarAlignment::Top => {
            let cx = cross + band_width / 2.;
            if value >= base {
                point(px(cx), px(value + TEXT_GAP))
            } else {
                point(px(cx), px(value - TEXT_HEIGHT))
            }
        }
        BarAlignment::Left => {
            let cy = cross + band_width / 2. - TEXT_SIZE / 2.;
            if value >= base {
                point(px(value + TEXT_GAP), px(cy))
            } else {
                point(px(value - TEXT_GAP), px(cy))
            }
        }
        BarAlignment::Right => {
            let cy = cross + band_width / 2. - TEXT_SIZE / 2.;
            if value <= base {
                point(px(value - TEXT_GAP), px(cy))
            } else {
                point(px(value + TEXT_GAP), px(cy))
            }
        }
    }
}
