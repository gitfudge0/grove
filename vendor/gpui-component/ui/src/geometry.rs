use std::fmt::{self, Debug, Display, Formatter};

use gpui::{AbsoluteLength, Axis, Length, Pixels};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// See also: [`Side`] for left/right only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Placement {
    #[serde(rename = "top")]
    Top,
    #[serde(rename = "bottom")]
    Bottom,
    #[serde(rename = "left")]
    Left,
    #[serde(rename = "right")]
    Right,
}

impl Display for Placement {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Placement::Top => write!(f, "Top"),
            Placement::Bottom => write!(f, "Bottom"),
            Placement::Left => write!(f, "Left"),
            Placement::Right => write!(f, "Right"),
        }
    }
}

impl Placement {
    #[inline]
    pub fn is_horizontal(&self) -> bool {
        match self {
            Placement::Left | Placement::Right => true,
            _ => false,
        }
    }

    #[inline]
    pub fn is_vertical(&self) -> bool {
        match self {
            Placement::Top | Placement::Bottom => true,
            _ => false,
        }
    }

    #[inline]
    pub fn axis(&self) -> Axis {
        match self {
            Placement::Top | Placement::Bottom => Axis::Vertical,
            Placement::Left | Placement::Right => Axis::Horizontal,
        }
    }
}


/// See also: [`Placement`] for all 4 edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    #[serde(rename = "left")]
    Left,
    #[serde(rename = "right")]
    Right,
}

impl Side {
    #[inline]
    pub fn is_left(&self) -> bool {
        matches!(self, Self::Left)
    }

    #[inline]
    pub fn is_right(&self) -> bool {
        matches!(self, Self::Right)
    }
}

pub trait AxisExt {
    fn is_horizontal(self) -> bool;
    fn is_vertical(self) -> bool;
}

impl AxisExt for Axis {
    #[inline]
    fn is_horizontal(self) -> bool {
        self == Axis::Horizontal
    }

    #[inline]
    fn is_vertical(self) -> bool {
        self == Axis::Vertical
    }
}

pub trait LengthExt {
    /// `None` for [`Length::Auto`].
    fn to_pixels(&self, base_size: AbsoluteLength, rem_size: Pixels) -> Option<Pixels>;
}

impl LengthExt for Length {
    fn to_pixels(&self, base_size: AbsoluteLength, rem_size: Pixels) -> Option<Pixels> {
        match self {
            Length::Auto => None,
            Length::Definite(len) => Some(len.to_pixels(base_size, rem_size)),
        }
    }
}

/// An extended, serializable version of [`gpui::Edges`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, Eq, PartialEq)]
#[repr(C)]
pub struct Edges<T: Clone + Debug + Default + PartialEq> {
    pub top: T,
    pub right: T,
    pub bottom: T,
    pub left: T,
}

impl<T> Edges<T>
where
    T: Clone + Debug + Default + PartialEq,
{
    pub fn all(value: T) -> Self {
        Self {
            top: value.clone(),
            right: value.clone(),
            bottom: value.clone(),
            left: value,
        }
    }
}
