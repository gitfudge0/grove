mod axis;
mod grid;
pub mod label;
pub mod scale;
pub mod shape;
pub mod tooltip;

pub use gpui_component_macros::IntoPlot;

use std::{fmt::Debug, ops::Add};

use gpui::{
    AnyElement, App, Bounds, ElementId, IntoElement, Path, PathBuilder, Pixels, Point, Window,
    point, px,
};

pub use axis::{AXIS_GAP, AxisLabelSide, AxisText, PlotAxis};
pub use grid::Grid;
pub use label::PlotLabel;

use tooltip::TooltipState;

pub trait Plot: IntoElement {
    /// Called during the element's prepaint phase; [`AnyElement::layout_as_root`]/[`prepaint_at`] are not legal from [`Plot::paint`]. Runs before [`Plot::tooltip_state`]/[`Plot::tooltip`].
    fn prepaint(
        &mut self,
        _bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Vec<AnyElement> {
        vec![]
    }

    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App);

    /// `Some(id)` opts into tooltips (must be unique among siblings); `None` disables them.
    fn id(&self) -> Option<ElementId> {
        None
    }

    /// `position` is origin-subtracted; only called while the cursor is inside `bounds`.
    fn tooltip_state(
        &self,
        _position: Point<Pixels>,
        _bounds: Bounds<Pixels>,
        _cx: &App,
    ) -> Option<TooltipState> {
        None
    }

    /// The overlay paints above the plot but below content drawn after it; [`tooltip::Tooltip`] defers its box to paint above everything.
    fn tooltip(
        &self,
        _state: &TooltipState,
        _cursor: Point<Pixels>,
        _bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<AnyElement> {
        None
    }
}

#[derive(Clone, Copy, Default)]
pub enum StrokeStyle {
    #[default]
    Natural,
    Linear,
    StepAfter,
}

pub fn origin_point<T>(x: T, y: T, origin: Point<T>) -> Point<T>
where
    T: Default + Clone + Debug + PartialEq + Add<Output = T>,
{
    point(x, y) + origin
}

pub fn polygon<T>(points: &[Point<T>], bounds: &Bounds<Pixels>) -> Option<Path<Pixels>>
where
    T: Default + Clone + Copy + Debug + Into<f32> + PartialEq,
{
    let mut path = PathBuilder::stroke(px(1.));
    let points = &points
        .iter()
        .map(|p| {
            point(
                px(p.x.into() + bounds.origin.x.as_f32()),
                px(p.y.into() + bounds.origin.y.as_f32()),
            )
        })
        .collect::<Vec<_>>();
    path.add_polygon(points, false);
    path.build().ok()
}
