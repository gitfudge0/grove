//! Small, reusable widget primitives — dots, dividers, buttons, hint pills,
//! and the modal panel chrome. None of these hold view state; they take the
//! data they need and return an `Element<Msg>`.
//!
//! The builders are grouped into submodules by what they are, and re-exported
//! flat at the `widgets::` level so every call site keeps its existing path.

mod buttons;
mod modal;
mod primitives;
mod rows;

pub(in crate::gui) use buttons::*;
pub(in crate::gui) use modal::*;
pub(in crate::gui) use primitives::*;
pub(in crate::gui) use rows::*;
