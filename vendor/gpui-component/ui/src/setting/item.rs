use gpui::{
    AnyElement, App, Axis, Div, InteractiveElement as _, IntoElement, ParentElement, SharedString,
    Stateful, Styled, Window, div, prelude::FluentBuilder as _,
};
use std::{any::TypeId, ops::Deref, rc::Rc};

use crate::{
    ActiveTheme as _, AxisExt, StyledExt as _,
    label::Label,
    setting::{
        AnySettingField, ElementField, RenderOptions,
        fields::{
            BoolField, DropdownField, NumberField, ResetHandler, SettingFieldRender, StringField,
        },
    },
    text::Text,
    v_flex,
};

#[derive(Clone)]
pub enum SettingItem {
    Item {
        title: SharedString,
        description: Option<Text>,
        keywords: Vec<SharedString>,
        layout: Axis,
        disabled: bool,
        field: Rc<dyn AnySettingField>,
    },
    Element {
        disabled: bool,
        keywords: Vec<SharedString>,
        /// First closure reports whether the item is dirty (reset button visibility); second performs the reset.
        reset_handler: Option<ResetHandler>,
        render: Rc<dyn Fn(&RenderOptions, &mut Window, &mut App) -> AnyElement + 'static>,
    },
}

impl SettingItem {
    pub fn new<F>(title: impl Into<SharedString>, field: F) -> Self
    where
        F: AnySettingField + 'static,
    {
        SettingItem::Item {
            title: title.into(),
            description: None,
            layout: Axis::Horizontal,
            disabled: false,
            keywords: Vec::new(),
            field: Rc::new(field),
        }
    }

    pub fn render<R, E>(render: R) -> Self
    where
        E: IntoElement,
        R: Fn(&RenderOptions, &mut Window, &mut App) -> E + 'static,
    {
        SettingItem::Element {
            disabled: false,
            keywords: Vec::new(),
            reset_handler: None,
            render: Rc::new(move |options, window, cx| {
                render(options, window, cx).into_any_element()
            }),
        }
    }

    /// Only applies to [`SettingItem::Element`]; the reset button shows while `is_dirty` is true and invokes `reset`.
    pub fn on_reset<D, R>(mut self, is_dirty: D, reset: R) -> Self
    where
        D: Fn(&App) -> bool + 'static,
        R: Fn(&mut Window, &mut App) + 'static,
    {
        match &mut self {
            SettingItem::Element { reset_handler, .. } => {
                *reset_handler = Some((Rc::new(is_dirty), Rc::new(reset)));
            }
            SettingItem::Item { .. } => {
                debug_assert!(
                    false,
                    "SettingItem::on_reset only applies to SettingItem::Element; \
                     use SettingField::default_value or SettingField::on_reset for a normal item"
                );
            }
        }
        self
    }

    /// Search-only, not rendered — e.g. "Enable Two-factor auth" made searchable via "MFA".
    pub fn keywords<I, S>(mut self, keywords: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<SharedString>,
    {
        let keywords: Vec<SharedString> = keywords.into_iter().map(Into::into).collect();
        match &mut self {
            SettingItem::Item { keywords: k, .. } => *k = keywords,
            SettingItem::Element { keywords: k, .. } => *k = keywords,
        }
        self
    }

    /// Renders with reduced opacity; for [`SettingItem::Element`] also forwarded via [`RenderOptions::disabled`].
    pub fn disabled(mut self, disabled: bool) -> Self {
        match &mut self {
            SettingItem::Item { disabled: d, .. } => *d = disabled,
            SettingItem::Element { disabled: d, .. } => *d = disabled,
        }
        self
    }

    /// Only applies to [`SettingItem::Item`].
    pub fn description(mut self, description: impl Into<Text>) -> Self {
        match &mut self {
            SettingItem::Item { description: d, .. } => {
                *d = Some(description.into());
            }
            SettingItem::Element { .. } => {}
        }
        self
    }

    /// Only applies to [`SettingItem::Item`].
    pub fn layout(mut self, layout: Axis) -> Self {
        match &mut self {
            SettingItem::Item { layout: l, .. } => {
                *l = layout;
            }
            SettingItem::Element { .. } => {}
        }
        self
    }

    pub(crate) fn is_match(&self, query: &str, cx: &App) -> bool {
        match self {
            SettingItem::Item {
                title,
                description,
                keywords,
                ..
            } => {
                let q = &query.to_lowercase();
                title.to_lowercase().contains(q)
                    || description
                        .as_ref()
                        .map_or(false, |d| d.get_text(cx).to_lowercase().contains(q))
                    || keywords.iter().any(|s| s.to_lowercase().contains(q))
            }
            SettingItem::Element { keywords, .. } => {
                let q = &query.to_lowercase();
                query.is_empty() || keywords.iter().any(|s| s.to_lowercase().contains(q))
            }
        }
    }

    pub(crate) fn is_resettable(&self, cx: &App) -> bool {
        match self {
            SettingItem::Item { field, .. } => field.is_resettable(cx),
            SettingItem::Element { reset_handler, .. } => reset_handler
                .as_ref()
                .is_some_and(|(is_dirty, _)| is_dirty(cx)),
        }
    }

    pub(crate) fn reset(&self, window: &mut Window, cx: &mut App) {
        match self {
            SettingItem::Item { field, .. } => field.reset(window, cx),
            SettingItem::Element { reset_handler, .. } => {
                if let Some((_, reset)) = reset_handler.as_ref() {
                    reset(window, cx);
                }
            }
        }
    }

    fn render_field(
        field: Rc<dyn AnySettingField>,
        options: RenderOptions,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        let field_type = field.field_type();
        let style = field.style().clone();
        let type_id = field.deref().type_id();
        let renderer: Box<dyn SettingFieldRender> = match type_id {
            t if t == std::any::TypeId::of::<bool>() => {
                Box::new(BoolField::new(field_type.is_switch()))
            }
            t if t == TypeId::of::<f64>() && field_type.is_number_input() => {
                Box::new(NumberField::new(field_type.number_input_options()))
            }
            t if t == TypeId::of::<SharedString>() && field_type.is_input() => {
                Box::new(StringField::<SharedString>::new())
            }
            t if t == TypeId::of::<String>() && field_type.is_input() => {
                Box::new(StringField::<String>::new())
            }
            t if t == TypeId::of::<SharedString>() && field_type.is_dropdown() => {
                Box::new(DropdownField::<SharedString>::new(
                    field_type.dropdown_options(),
                    field_type.dropdown_scrollable(),
                ))
            }
            t if t == TypeId::of::<String>() && field_type.is_dropdown() => {
                Box::new(DropdownField::<String>::new(
                    field_type.dropdown_options(),
                    field_type.dropdown_scrollable(),
                ))
            }
            _ if field_type.is_element() => Box::new(ElementField::new(field_type.element())),
            _ => unimplemented!("Unsupported setting type: {}", field.deref().type_name()),
        };

        renderer.render(field, &options, &style, window, cx)
    }

    pub(super) fn render_item(
        self,
        options: &RenderOptions,
        window: &mut Window,
        cx: &mut App,
    ) -> Stateful<Div> {
        div()
            .id(SharedString::from(format!("item-{}", options.item_ix)))
            .w_full()
            .child(match self {
                SettingItem::Item {
                    title,
                    description,
                    layout,
                    disabled,
                    field,
                    ..
                } => {
                    let layout = if options.layout.is_vertical() {
                        Axis::Vertical
                    } else {
                        layout
                    };

                    div()
                        .w_full()
                        .overflow_hidden()
                        .when(disabled, |this| this.opacity(0.5))
                        .map(|this| {
                            if layout.is_horizontal() {
                                this.h_flex().justify_between().items_start()
                            } else {
                                this.v_flex()
                            }
                        })
                        .gap_3()
                        .child(
                            v_flex()
                                .map(|this| {
                                    if layout.is_horizontal() {
                                        this.flex_1().max_w_3_5()
                                    } else {
                                        this.w_full()
                                    }
                                })
                                .gap_1()
                                .child(Label::new(title).text_sm())
                                .when_some(description, |this, description| {
                                    this.child(
                                        div()
                                            .size_full()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(description),
                                    )
                                }),
                        )
                        .child(div().id("field").child(Self::render_field(
                            field,
                            RenderOptions {
                                layout,
                                disabled,
                                ..*options
                            },
                            window,
                            cx,
                        )))
                        .into_any_element()
                }
                SettingItem::Element {
                    disabled, render, ..
                } => div()
                    .w_full()
                    .when(disabled, |this| this.opacity(0.5))
                    .child((render)(
                        &RenderOptions {
                            disabled,
                            ..*options
                        },
                        window,
                        cx,
                    ))
                    .into_any_element(),
            })
    }
}
