//! Typed application bindings for the certified dense-action recipe.
use gpui::{App, ClickEvent, ElementId, SharedString, Window};
use gpui_component::Icon;
use std::rc::Rc;

pub(crate) type Action = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

pub(crate) struct DenseActionBindings {
    pub id: ElementId,
    pub label: SharedString,
    pub icon: Option<Icon>,
    pub disabled: bool,
    pub loading: bool,
    pub on_activate: Action,
}

mod generated {
    include!("generated_dense_action.rs");
}
pub(crate) use generated::dense_action;
