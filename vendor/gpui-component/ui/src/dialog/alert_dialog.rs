use gpui::{
    AnyElement, App, ClickEvent, InteractiveElement as _, IntoElement, MouseButton, ParentElement,
    Pixels, RenderOnce, StyleRefinement, Styled, Window, div, prelude::FluentBuilder as _,
};

use crate::{
    StyledExt as _, WindowExt as _,
    dialog::{
        Dialog, DialogButtonProps, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
    },
    h_flex, v_flex,
};

/// A modal dialog that interrupts the user and expects a response; built on [`Dialog`] with center-aligned footer buttons and an optional icon.
///
/// # Examples
///
/// ## Imperative API (using WindowExt)
///
/// ```ignore
/// use gpui_component::{AlertDialog, alert::AlertVariant};
///
/// // Using WindowExt trait
/// window.open_alert_dialog(cx, |alert, _, _| {
///     alert
///         .title("Unsaved Changes")
///         .description("You have unsaved changes. Are you sure you want to leave?")
///         .show_cancel(true)
/// });
/// ```
///
/// ## Declarative API (using trigger and content)
///
/// ```ignore
/// use gpui_component::{AlertDialog, DialogHeader, DialogTitle, DialogDescription, DialogFooter};
///
/// AlertDialog::new(cx)
///     .trigger(Button::new("delete").label("Delete"))
///     .content(|content, _, cx| {
///         content
///             .child(
///                 DialogHeader::new()
///                     .items_center()
///                     .child(DialogTitle::new().child("Delete File"))
///                     .child(DialogDescription::new().child("Are you sure?"))
///             )
///             .child(
///                 DialogFooter::new()
///                     .justify_center()
///                     .child(Button::new("cancel").label("Cancel"))
///                     .child(Button::new("confirm").label("Delete"))
///             )
///     })
/// ```
#[derive(IntoElement)]
pub struct AlertDialog {
    base: Dialog,
    trigger: Option<AnyElement>,
    icon: Option<AnyElement>,
    title: Option<AnyElement>,
    description: Option<AnyElement>,
    button_props: DialogButtonProps,
    children: Vec<AnyElement>,
}

impl AlertDialog {
    /// Not overlay-closable by default; change with `.overlay_closable(true)`.
    pub fn new(cx: &mut App) -> Self {
        Self {
            base: Dialog::new(cx).overlay_closable(false).close_button(false),
            trigger: None,
            icon: None,
            title: None,
            description: None,
            button_props: DialogButtonProps::default(),
            children: Vec::new(),
        }
    }

    /// Adds a Cancel button alongside the default OK button.
    pub fn confirm(mut self) -> Self {
        self.button_props.show_cancel = true;
        self
    }

    /// Renders as a clickable trigger that opens the dialog; use with `.content()` — `title`/`description`/`icon`/`button_props` are ignored when a trigger is set.
    pub fn trigger(mut self, trigger: impl IntoElement) -> Self {
        self.trigger = Some(trigger.into_any_element());
        self
    }

    /// Declarative content builder using `DialogHeader`/`DialogTitle`/`DialogDescription`/`DialogFooter`; typically paired with `.trigger()`.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// AlertDialog::new(cx)
    ///     .trigger(Button::new("delete").label("Delete"))
    ///     .content(|content, _, cx| {
    ///         content
    ///             .child(DialogHeader::new().child(DialogTitle::new().child("Confirm")))
    ///             .child(DialogFooter::new().child(Button::new("ok").label("OK")))
    ///     })
    /// ```
    pub fn content<F>(mut self, builder: F) -> Self
    where
        F: Fn(crate::dialog::DialogContent, &mut Window, &mut App) -> crate::dialog::DialogContent
            + 'static,
    {
        self.base = self.base.content(builder);
        self
    }

    /// Declarative footer builder; if unset, a default OK/Cancel footer is used.
    pub fn footer(mut self, footer: impl IntoElement) -> Self {
        self.base = self.base.footer(footer);
        self
    }

    #[track_caller]
    fn debug_assert_no_trigger(&self) {
        debug_assert!(
            self.trigger.is_none() && self.base.content_builder.is_none(),
            "Cannot set this property when trigger is used. Use content() to define dialog content instead."
        );
    }

    #[track_caller]
    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.debug_assert_no_trigger();
        self.icon = Some(icon.into_any_element());
        self
    }

    #[track_caller]
    pub fn title(mut self, title: impl IntoElement) -> Self {
        self.debug_assert_no_trigger();
        self.title = Some(title.into_any_element());
        self
    }

    #[track_caller]
    pub fn description(mut self, description: impl IntoElement) -> Self {
        self.debug_assert_no_trigger();
        self.description = Some(description.into_any_element());
        self
    }

    /// Configures button text, variants, and visibility.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// alert.button_props(
    ///     DialogButtonProps::default()
    ///         .ok_text("Delete")
    ///         .ok_variant(ButtonVariant::Danger)
    ///         .cancel_text("Keep")
    ///         .show_cancel(true)
    /// )
    /// ```
    #[track_caller]
    pub fn button_props(mut self, button_props: DialogButtonProps) -> Self {
        self.debug_assert_no_trigger();
        self.button_props = button_props;
        self
    }

    /// Defaults to 420px.
    pub fn width(mut self, width: impl Into<Pixels>) -> Self {
        self.base = self.base.width(width);
        self
    }

    pub fn show_cancel(mut self, show_cancel: bool) -> Self {
        self.button_props = self.button_props.show_cancel(show_cancel);
        self
    }

    /// Defaults to `false`.
    pub fn overlay_closable(mut self, overlay_closable: bool) -> Self {
        self.base = self.base.overlay_closable(overlay_closable);
        self
    }

    /// Defaults to `false`.
    pub fn close_button(mut self, close_button: bool) -> Self {
        self.base = self.base.close_button(close_button);
        self
    }

    /// Defaults to `true`.
    pub fn keyboard(mut self, keyboard: bool) -> Self {
        self.base = self.base.keyboard(keyboard);
        self
    }

    /// Called after [`Self::on_action`] or [`Self::on_cancel`].
    pub fn on_close(
        mut self,
        on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.base = self.base.on_close(on_close);
        self
    }

    /// Return `true` to close the dialog, `false` to keep it open.
    pub fn on_ok(
        mut self,
        on_ok: impl Fn(&ClickEvent, &mut Window, &mut App) -> bool + 'static,
    ) -> Self {
        self.button_props = self.button_props.on_ok(on_ok);
        self
    }

    /// Return `true` to close the dialog, `false` to keep it open.
    pub fn on_cancel(
        mut self,
        on_cancel: impl Fn(&ClickEvent, &mut Window, &mut App) -> bool + 'static,
    ) -> Self {
        self.button_props = self.button_props.on_cancel(on_cancel);
        self
    }

    pub(crate) fn into_dialog(self, window: &mut Window, cx: &mut App) -> Dialog {
        let button_props = self.button_props.clone();
        let has_title = self.icon.is_some() || self.title.is_some();
        let has_header = has_title || self.description.is_some();
        let has_footer = self.base.footer.is_some();

        self.base
            .button_props(button_props.clone())
            .alert_dialog_role()
            .when(has_header, |this| {
                this.header(
                    DialogHeader::new().child(
                        h_flex()
                            .gap_2()
                            .items_start()
                            .when_some(self.icon, |row, icon| row.child(icon))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .when_some(self.title, |this, title| {
                                        this.child(DialogTitle::new().child(title))
                                    })
                                    .when_some(self.description, |this, desc| {
                                        this.child(DialogDescription::new().child(desc))
                                    }),
                            ),
                    ),
                )
            })
            .children(self.children)
            .when(!has_footer, |this| {
                this.footer(
                    DialogFooter::new()
                        .when(button_props.show_cancel, |this| {
                            this.child(button_props.render_cancel(window, cx))
                        })
                        .child(button_props.render_ok(window, cx)),
                )
            })
    }
}

impl Styled for AlertDialog {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.base.style
    }
}

impl ParentElement for AlertDialog {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl AlertDialog {
    fn render_trigger(self, trigger: AnyElement, _: &mut Window, _: &mut App) -> AnyElement {
        let content_builder = self.base.content_builder.clone();
        let style = self.base.style.clone();
        let props = self.base.props.clone();
        let button_props = self.button_props.clone();

        div()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                let content_builder = content_builder.clone();
                let style = style.clone();
                let props = props.clone();
                let button_props = button_props.clone();
                window.open_dialog(cx, move |dialog, _, _| {
                    dialog
                        .refine_style(&style)
                        .button_props(button_props.clone())
                        .with_props(props.clone())
                        .when_some(content_builder.clone(), |this, content_builder| {
                            this.content(move |content, window, cx| {
                                content_builder(content, window, cx)
                            })
                        })
                });
                cx.stop_propagation();
            })
            .child(trigger)
            .into_any_element()
    }
}

impl RenderOnce for AlertDialog {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if let Some(trigger) = self.trigger.take() {
            self.render_trigger(trigger, window, cx)
        } else {
            self.into_dialog(window, cx).into_any_element()
        }
    }
}
