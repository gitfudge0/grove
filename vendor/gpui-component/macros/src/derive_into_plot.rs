use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, parse_macro_input};

pub fn derive_into_plot(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let type_name = &ast.ident;
    let (impl_generics, type_generics, where_clause) = ast.generics.split_for_impl();

    let expanded = quote! {
        impl #impl_generics gpui::IntoElement for #type_name #type_generics #where_clause {
            type Element = Self;

            fn into_element(self) -> Self::Element {
                self
            }
        }

        impl #impl_generics #type_name #type_generics #where_clause {
            #[doc(hidden)]
            fn __plot_tooltip_cursor(
                global_id: &gpui::GlobalElementId,
                window: &mut gpui::Window,
            ) -> std::rc::Rc<std::cell::Cell<Option<gpui::Point<gpui::Pixels>>>> {
                window.with_element_state(global_id, |prev, _| {
                    let cell: std::rc::Rc<
                        std::cell::Cell<Option<gpui::Point<gpui::Pixels>>>,
                    > = prev.unwrap_or_default();
                    (cell.clone(), cell)
                })
            }
        }

        impl #impl_generics gpui::Element for #type_name #type_generics #where_clause {
            type RequestLayoutState = ();
            // Carries the hitbox, prepainted children, and prepainted tooltip overlay from `prepaint` to `paint`.
            type PrepaintState = (
                Option<gpui::Hitbox>,
                Vec<gpui::AnyElement>,
                Option<gpui::AnyElement>,
            );

            fn id(&self) -> Option<gpui::ElementId> {
                // `Some` opts in to interactive tooltips; `None` is a pure, non-interactive plot.
                <Self as Plot>::id(self)
            }

            fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
                None
            }

            fn request_layout(
                &mut self,
                _: Option<&gpui::GlobalElementId>,
                _: Option<&gpui::InspectorElementId>,
                window: &mut gpui::Window,
                cx: &mut gpui::App,
            ) -> (gpui::LayoutId, Self::RequestLayoutState) {
                let style = gpui::Style {
                    size: gpui::Size::full(),
                    ..Default::default()
                };

                (window.request_layout(style, None, cx), ())
            }

            fn prepaint(
                &mut self,
                global_id: Option<&gpui::GlobalElementId>,
                _: Option<&gpui::InspectorElementId>,
                bounds: gpui::Bounds<gpui::Pixels>,
                _: &mut Self::RequestLayoutState,
                window: &mut gpui::Window,
                cx: &mut gpui::App,
            ) -> Self::PrepaintState {
                // Laid out here since `layout_as_root`/`prepaint_at` are prepaint-only, above the early return so plots without an id still get children.
                let children = <Self as Plot>::prepaint(self, bounds, window, cx);

                let Some(global_id) = global_id else {
                    return (None, children, None);
                };

                // `Hitbox::is_hovered` is false under an occluding hitbox (e.g. an open popup), unlike a plain bounds test.
                let hitbox = window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal);

                let overlay = (|| {
                    // The cached cell is one frame stale during scrolling, so derive position from the live mouse and this frame's bounds to avoid jitter.
                    Self::__plot_tooltip_cursor(global_id, window).get()?;
                    let mouse = window.mouse_position();
                    if !bounds.contains(&mouse) {
                        return None;
                    }
                    let position = mouse - bounds.origin;
                    let state = <Self as Plot>::tooltip_state(self, position, bounds, cx)?;

                    // The tooltip box defers itself (`plot::tooltip::Tooltip`) to paint above sibling content since it can extend past the plot bounds.
                    let mut overlay = <Self as Plot>::tooltip(self, &state, position, bounds, window, cx)?;
                    overlay.prepaint_as_root(bounds.origin, bounds.size.into(), window, cx);
                    Some(overlay)
                })();

                (Some(hitbox), children, overlay)
            }

            fn paint(
                &mut self,
                global_id: Option<&gpui::GlobalElementId>,
                _: Option<&gpui::InspectorElementId>,
                bounds: gpui::Bounds<gpui::Pixels>,
                _: &mut Self::RequestLayoutState,
                prepaint: &mut Self::PrepaintState,
                window: &mut gpui::Window,
                cx: &mut gpui::App,
            ) {
                <Self as Plot>::paint(self, bounds, window, cx);

                let (hitbox, children, overlay) = prepaint;

                for child in children.iter_mut() {
                    child.paint(window, cx);
                }

                if let (Some(global_id), Some(hitbox)) = (global_id, hitbox.as_ref()) {
                    let cell = Self::__plot_tooltip_cursor(global_id, window);
                    let hitbox = hitbox.clone();

                    // Scrolling moves the plot with no MouseMoveEvent, so re-derive every frame; a visibility flip alone needs `request_animation_frame`.
                    let next = if hitbox.is_hovered(window) {
                        Some(window.mouse_position() - bounds.origin)
                    } else {
                        None
                    };
                    if cell.get() != next {
                        let visibility_changed = cell.get().is_some() != next.is_some();
                        cell.set(next);
                        if visibility_changed {
                            window.request_animation_frame();
                        }
                    }

                    window.on_mouse_event(
                        move |e: &gpui::MouseMoveEvent, _, window: &mut gpui::Window, _| {
                            let next = if hitbox.is_hovered(window) {
                                Some(e.position - bounds.origin)
                            } else {
                                None
                            };

                            if cell.get() != next {
                                cell.set(next);
                                window.refresh();
                            }
                        },
                    );
                }

                if let Some(overlay) = overlay.as_mut() {
                    overlay.paint(window, cx);
                }
            }
        }
    };

    TokenStream::from(expanded)
}
