//! Production composition for typed references. Capture only decorates these
//! same elements; application data, focus and action effects retain their owners.
use crate::{
    theme as c,
    views::{components, rpx, tokens::*},
};
use gpui::{div, prelude::*, AnyElement, App, Hsla, SharedString, Window};
use gpui_component::button::Button;
pub(crate) type Action = super::runtime::Action;
#[cfg(test)]
pub(crate) type FixtureAction = std::rc::Rc<dyn Fn(&str, &mut Window, &mut App)>;

pub fn measure(id: &str, kind: &str, fill: bool, child: AnyElement) -> AnyElement {
    let element = div()
        .flex()
        .when(fill, |element| element.w_full().min_w_0())
        .when(!fill, gpui::Styled::flex_shrink_0);
    #[cfg(test)]
    let element = super::native::instrument_node(element, id, kind);
    #[cfg(not(test))]
    let _ = kind;
    let selector = format!("ui-reference-{id}");
    element
        .child(child)
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .into_any_element()
}
pub fn catalog_icon(id: &str, name: &str, color: Hsla) -> impl IntoElement {
    #[cfg(test)]
    super::native::observe_icon(id, name, color);
    #[cfg(not(test))]
    let _ = id;
    crate::icons::icon(name, ICON_SM, color)
}
pub fn surface(id: &str, child: gpui::Div) -> gpui::Div {
    let fill = c::BG_RAIL();
    #[cfg(test)]
    super::native::observe_surface(id, fill);
    #[cfg(not(test))]
    let _ = id;
    child.bg(fill)
}
pub fn text(
    id: &str,
    value: &str,
    size: f32,
    color: &str,
    _window: &Window,
    _cx: &App,
) -> impl IntoElement {
    let content = gpui::StyledText::new(value.to_owned());
    let element = div()
        .font_family(crate::fonts::UI_FAMILY)
        .text_size(rpx(size))
        .line_height(rpx(size))
        .whitespace_nowrap()
        .text_color(match color {
            "supporting" => c::FG_DIM(),
            "error" => c::RED(),
            "success" => c::GREEN(),
            "background" => c::BG(),
            "rail" => c::BG_RAIL(),
            "hover" => c::BG_HOVER(),
            "border" => c::BORDER(),
            "field" => c::FIELD_FILL(),
            _ => c::FG(),
        });
    #[cfg(test)]
    let element = super::native::instrument_text(element, id, value, content.layout().clone());
    #[cfg(not(test))]
    let _ = id;
    element.child(content)
}
pub fn action(
    id: &str,
    label: &str,
    icon: Option<&str>,
    state: &str,
    activate: Action,
    window: &Window,
    cx: &App,
) -> Button {
    #[cfg(test)]
    let rendered_label = super::native::action_label(label);
    #[cfg(not(test))]
    let rendered_label = label.to_owned();
    let button = super::runtime::dense_action(
        super::runtime::DenseActionBindings {
            id: SharedString::from(id.to_owned()).into(),
            label: rendered_label.into(),
            icon: icon.map(|name| {
                gpui_component::Icon::default()
                    .path(SharedString::from(format!("icons/{name}.svg")))
            }),
            disabled: state == "disabled",
            loading: state == "loading",
            on_activate: activate,
        },
        window,
        cx,
    );
    #[cfg(test)]
    let button = super::native::instrument_action(button, id, label, icon, window);
    button
}
pub fn checkout(
    id: &str,
    label: &str,
    icon: &str,
    action: AnyElement,
    _window: &Window,
    _cx: &App,
) -> impl IntoElement {
    let label_id = format!("{id}-label");
    let content = gpui::StyledText::new(label.to_owned());
    let element = components::checkout_register_label();
    #[cfg(test)]
    let element =
        super::native::instrument_text(element, &label_id, label, content.layout().clone());
    let label_node = element
        .child(content)
        .id(SharedString::from(format!("ui-reference-{label_id}")))
        .debug_selector({
            let label_id = label_id.clone();
            move || format!("ui-reference-{label_id}")
        });
    let row = components::checkout_register_frame();
    #[cfg(test)]
    let row = super::native::instrument_checkout(row, &label_id);
    let icon_id = format!("{id}-icon");
    row.child(components::checkout_register_leading().child(measure(
        &icon_id,
        "icon",
        false,
        catalog_icon(&icon_id, icon, c::FG_DIM()).into_any_element(),
    )))
    .child(label_node)
    .child(action)
}
